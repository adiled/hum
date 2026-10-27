package hum.hive.wifip2p

import java.io.Closeable
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * A humd on a loopback port that attunes as humd does: a breath with
 * sigils [] on connect, an echo of every tone's rid, then a breath that
 * admits the hello, or an echo with ok false when [refuse] names a reason.
 */
class FakeHumd(private val refuse: (Tone) -> String? = { null }) : Closeable {
    private val server = ServerSocket(0, 8, InetAddress.getLoopbackAddress())
    val dial = dial(Humd.Tcp("127.0.0.1", server.localPort))
    val hellos = LinkedBlockingQueue<Tone>()
    val heard = LinkedBlockingQueue<Tone>()
    @Volatile var connections = 0
    @Volatile private var socket: Socket? = null

    init {
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val socket = try { server.accept() } catch (_: IOException) { break }
                this.socket = socket
                connections++
                thread(isDaemon = true) { serve(socket) }
            }
        }
    }

    private fun serve(socket: Socket) {
        try {
            say(socket, tone(Chi.BREATH, "from" to "daemon", "sigils" to emptyList<Any>(), "protoVersion" to THRUM_VERSION))
            val lines = Lines(socket.getInputStream())
            while (true) {
                val tone = parseTone(lines.next() ?: break) ?: continue
                val rid = tone.str("rid")
                if (rid != null && tone.str("chi") != Chi.ECHO) say(socket, tone(Chi.ECHO, "rid" to rid, "ok" to true))
                if (tone.str("chi") == Chi.HELLO) {
                    hellos += tone
                    val why = refuse(tone)
                    if (why == null) {
                        say(socket, tone(Chi.BREATH, "sigils" to emptyMap<String, Any>(), "models" to listOf(mapOf("id" to "echo", "hive" to "echo"))))
                    } else {
                        say(socket, tone(Chi.ECHO, "rid" to rid, "ok" to false, "error" to why))
                        socket.close()
                    }
                } else {
                    heard += tone
                }
            }
        } catch (_: IOException) {
        } finally {
            socket.close()
        }
    }

    private fun say(socket: Socket, tone: Tone) = synchronized(socket) {
        socket.getOutputStream().write(line(tone))
        socket.getOutputStream().flush()
    }

    fun say(tone: Tone) = say(socket!!, tone)

    fun drop() = socket?.close()

    fun nextHello(): Tone = hellos.poll(5, TimeUnit.SECONDS) ?: error("no hello")

    fun nextHeard(): Tone = heard.poll(5, TimeUnit.SECONDS) ?: error("humd heard nothing")

    override fun close() {
        server.close()
        socket?.close()
    }
}
