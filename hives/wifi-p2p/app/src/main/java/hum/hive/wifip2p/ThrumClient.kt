// The bee's connection to its humd, kept as hum's client keeps one
// (THRUM.md, Speaking the wire yourself). It dials, sends the hello signed
// under a fresh rid, and waits for humd's verdict: an echo of the hello and
// then a breath admit the bee, an echo with ok false refuses it, and a
// refusal is final. When the connection closes it dials again, 250 ms
// doubling to 30 s with a fifth of jitter either way. A tone sent while
// humd is away is dropped.
//
// humd admits bees on its Unix socket only. An address that starts with /
// is that socket, dialed with LocalSocket, which reaches it only where the
// app may open the file; host:port is a TCP stream that something on the
// phone carries to the socket.

package hum.hive.wifip2p

import android.net.LocalSocket
import android.net.LocalSocketAddress
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket

sealed interface Humd {
    data class Unix(val path: String) : Humd
    data class Tcp(val host: String, val port: Int) : Humd

    companion object {
        fun parse(address: String): Humd? {
            val at = address.trim()
            if (at.startsWith("/")) return Unix(at)
            val colon = at.lastIndexOf(':')
            if (colon <= 0) return null
            val host = at.substring(0, colon).removeSurrounding("[", "]")
            val port = at.substring(colon + 1).toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
            return if (host.isEmpty()) null else Tcp(host, port)
        }
    }
}

class Link(val input: InputStream, val output: OutputStream, private val shut: () -> Unit) {
    fun close() {
        runCatching(shut)
    }
}

fun interface Dial {
    fun open(): Link
}

private const val DIAL_MS = 5_000

fun dial(humd: Humd): Dial = when (humd) {
    is Humd.Unix -> Dial {
        val socket = LocalSocket()
        try {
            socket.connect(LocalSocketAddress(humd.path, LocalSocketAddress.Namespace.FILESYSTEM))
        } catch (e: IOException) {
            socket.close()
            throw e
        }
        Link(socket.inputStream, socket.outputStream) {
            runCatching { socket.shutdownInput() }
            runCatching { socket.shutdownOutput() }
            socket.close()
        }
    }
    is Humd.Tcp -> Dial {
        val socket = Socket()
        try {
            socket.connect(InetSocketAddress(humd.host, humd.port), DIAL_MS)
            socket.tcpNoDelay = true
        } catch (e: IOException) {
            socket.close()
            throw e
        }
        Link(socket.getInputStream(), socket.getOutputStream(), socket::close)
    }
}

class ThrumClient(
    private val dial: Dial,
    private val hive: String,
    private val role: String,
    private val key: BeeKey,
    private val hello: List<Pair<String, Any?>>,
    private val verdictMs: Long = VERDICT_MS,
    private val heard: (Event) -> Unit,
) {
    sealed interface Event {
        data object Nestled : Event
        data object Astray : Event
        data class Heard(val tone: Tone) : Event
        data class Refused(val why: String) : Event
        data class Failing(val tries: Int, val why: String) : Event
    }

    private sealed interface End {
        data object Astray : End
        data class Refused(val why: String) : End
        data class Unanswered(val why: String) : End
    }

    @Volatile private var nestled: SendChannel<Tone>? = null
    @Volatile private var link: Link? = null
    private var job: Job? = null

    fun start(scope: CoroutineScope) {
        job = scope.launch(Dispatchers.IO) { live() }
    }

    fun stop() {
        job?.cancel()
        link?.close()
    }

    /** Sends a tone to humd: false, and the tone dropped, while humd is away. */
    fun send(tone: Tone): Boolean = nestled?.trySend(tone)?.isSuccess == true

    private suspend fun live() {
        var tries = 0
        while (true) {
            val why = try {
                when (val end = attune(dial.open().also { link = it })) {
                    is End.Refused -> {
                        heard(Event.Refused(end.why))
                        return
                    }
                    End.Astray -> {
                        tries = 0
                        heard(Event.Astray)
                        "humd went away"
                    }
                    is End.Unanswered -> end.why
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                e.message ?: e.javaClass.simpleName
            }
            tries++
            if (tries == QUIET_TRIES || (tries > QUIET_TRIES && tries and (tries - 1) == 0)) {
                heard(Event.Failing(tries, why))
            }
            delay(backoff(tries - 1))
        }
    }

    private suspend fun attune(link: Link): End = coroutineScope {
        val queue = Channel<Tone>(QUEUE)
        val writer = launch {
            try {
                for (tone in queue) {
                    link.output.write(line(tone))
                    link.output.flush()
                }
            } catch (_: IOException) {
                link.close()
            }
        }
        val helloRid = rid()
        queue.trySend(signedHello(helloRid))
        val verdict = launch {
            delay(verdictMs)
            link.close()
        }
        var acked = false
        var admitted = false
        try {
            val lines = Lines(link.input)
            while (true) {
                val tone = parseTone(lines.next() ?: break) ?: continue
                val chi = tone.str("chi")
                when {
                    chi == Chi.ECHO && tone.str("rid") == helloRid ->
                        if (tone.bool("ok") == false) {
                            return@coroutineScope End.Refused(tone.str("error") ?: "humd refused the hello")
                        } else {
                            acked = true
                        }
                    admitted -> heard(Event.Heard(tone))
                    chi == Chi.BREATH && acked -> {
                        admitted = true
                        verdict.cancel()
                        nestled = queue
                        heard(Event.Nestled)
                        heard(Event.Heard(tone))
                    }
                }
            }
            if (admitted) End.Astray else End.Unanswered("humd gave no verdict")
        } catch (e: IOException) {
            if (admitted) End.Astray else End.Unanswered(e.message ?: "humd gave no verdict")
        } finally {
            nestled = null
            queue.close()
            verdict.cancel()
            link.close()
            writer.cancel()
        }
    }

    private fun signedHello(rid: String): Tone = tone(
        Chi.HELLO,
        "rid" to rid,
        "hive" to hive,
        "role" to role,
        "protoVersion" to THRUM_VERSION,
        *hello.toTypedArray(),
        *key.proof(System.currentTimeMillis()).toTypedArray(),
    )

    companion object {
        const val VERDICT_MS = 10_000L
        const val QUIET_TRIES = 8
        private const val FIRST_MS = 250L
        private const val MOST_MS = 30_000L
        private const val QUEUE = 1024

        fun backoff(tries: Int): Long {
            val base = minOf(MOST_MS, FIRST_MS shl minOf(tries, 16))
            return (base * (0.8 + Math.random() * 0.4)).toLong()
        }
    }
}
