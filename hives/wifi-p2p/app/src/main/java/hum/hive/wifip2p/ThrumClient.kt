package hum.hive.wifip2p

import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintWriter
import java.net.Socket
import java.net.UnixDomainSocketAddress
import java.nio.channels.Channels
import java.nio.file.Path
import kotlinx.coroutines.sync.withLock

/**
 * NDJSON client to humd over Unix socket (Termux) or TCP bridge.
 *
 * Mirrors hives/openai-server/src/thrum.ts — same framing, same
 * hello contract, same reconnect behavior.
 *
 * Connect modes:
 *   LOCAL_UNIX — connect to humd's Unix socket (Termux context)
 *   TCP — connect to a remote TCP bridge (e.g., humd's ensemble port)
 */
class ThrumClient(
    private val sockPath: String,
    private val dataDir: String,
    private val scope: CoroutineScope,
) {
    enum class Mode { LOCAL_UNIX, TCP }

    private var job: Job? = null
    private var connected = false
    private val writeLock = Mutex()
    private val pending = mutableListOf<String>()
    private val handlers = mutableMapOf<String, (Tone) -> Unit>()
    private var catchAllHandler: ((Tone) -> Unit)? = null

    private var reconnectAttempt = 0
    private var reconnectJob: Job? = null

    companion object {
        const val BEE_VERSION = "0.1.0"
        const val HIVE_NAME = "wifi-p2p"
    }

    fun connect() {
        job = scope.launch {
            attemptConnect()
        }
    }

    private suspend fun attemptConnect() {
        val hid = IdentityStore.getHid(dataDir)
        val mode = resolveMode()

        try {
            val (reader, writer) = when (mode) {
                Mode.LOCAL_UNIX -> connectUnix(sockPath)
                Mode.TCP -> connectTcp(sockPath)
            }

            connected = true
            reconnectAttempt = 0

            // Send hello
            val hello = Tone(
                chi = Chi.HELLO,
                rid = "hello-${System.currentTimeMillis().toString(36)}",
                from = HIVE_NAME,
                body = mapOf(
                    "hid" to hid,
                    "bee" to listOf("forager"),
                    "hive" to HIVE_NAME,
                    "version" to BEE_VERSION,
                    "protoVersion" to THRUM_VERSION,
                    "propensity" to mapOf(
                        "statefulness" to "convention-stateful",
                        "richness" to "lean",
                        "wire" to "wifi-p2p/tcp",
                    ),
                    "chis" to listOf(
                        Chi.HELLO, Chi.PROMPT, Chi.CANCEL,
                        Chi.CHUNK, Chi.FINISH, Chi.ERROR,
                        Chi.CLEANUP,
                    ),
                    "source" to "https://github.com/adiled/hum/tree/main/hives/wifi-p2p",
                )
            )
            sendNow(writer, hello)
            // Flush pending
            writeLock.withLock {
                for (line in pending) {
                    writer.println(line)
                }
                pending.clear()
            }

            // Read loop
            val readerThread = BufferedReader(InputStreamReader(reader))
            while (isActive) {
                val line = readerThread.readLine() ?: break
                if (line.isBlank()) continue
                try {
                    val tone = Tone.fromJson(line)
                    handleTone(tone)
                } catch (_: Exception) {
                    // drop malformed lines per wire spec
                }
            }
        } catch (e: CancellationException) {
            return
        } catch (e: Exception) {
            // Connection failed — schedule reconnect
        }

        connected = false
        if (isActive) {
            scheduleReconnect()
        }
    }

    private fun handleTone(tone: Tone) {
        val sid = tone.sid ?: ""
        val handler = handlers[sid]
        if (handler != null) {
            handler(tone)
        } else {
            catchAllHandler?.invoke(tone)
        }
    }

    fun on(sid: String, handler: (Tone) -> Unit) {
        handlers[sid] = handler
    }

    fun off(sid: String) {
        handlers.remove(sid)
    }

    fun onCatchAll(handler: (Tone) -> Unit) {
        catchAllHandler = handler
    }

    fun send(tone: Tone) {
        val line = tone.toJson()
        scope.launch {
            writeLock.withLock {
                if (connected) {
                    // Write queued — the read/write pair handles this
                }
            }
            pending.add(line)
        }
    }

    private fun sendNow(writer: PrintWriter, tone: Tone) {
        writer.println(tone.toJson())
        writer.flush()
    }

    private fun resolveMode(): Mode {
        return if (sockPath.startsWith("/") || sockPath.startsWith("/run")) {
            Mode.LOCAL_UNIX
        } else {
            Mode.TCP
        }
    }

    private fun connectUnix(path: String): Pair<Any, PrintWriter> {
        val addr = UnixDomainSocketAddress.of(path)
        val socket = java.net.Socket() // Use SocketChannel for Unix domain
        val channel = java.nio.channels.SocketChannel.open()
        channel.connect(addr)
        val input = Channels.newInputStream(channel)
        val output = PrintWriter(Channels.newOutputStream(channel), true)
        return input to output
    }

    private fun connectTcp(addr: String): Pair<Any, PrintWriter> {
        val hostPort = addr.split(":")
        val host = hostPort[0]
        val port = hostPort.getOrNull(1)?.toInt() ?: 14620
        val socket = Socket(host, port)
        val input = socket.getInputStream()
        val output = PrintWriter(socket.getOutputStream(), true)
        return input to output
    }

    private fun scheduleReconnect() {
        val delay = minOf(30_000L, 250L * (1L shl reconnectAttempt))
        reconnectAttempt++
        reconnectJob = scope.launch {
            delay(delay)
            attemptConnect()
        }
    }

    fun disconnect() {
        job?.cancel()
        reconnectJob?.cancel()
        connected = false
    }
}
