package hum.hive.wifip2p

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class ThrumClientTest {
    private val scope = CoroutineScope(Dispatchers.IO)
    private val events = LinkedBlockingQueue<ThrumClient.Event>()
    private val key = BeeKey(ByteArray(32) { 7 })

    @After
    fun stop() = scope.cancel()

    private fun client(dial: Dial, verdictMs: Long = ThrumClient.VERDICT_MS) =
        ThrumClient(dial, HIVE, Role.FORAGER, key, HELLO, verdictMs) { events += it }.also { it.start(scope) }

    private fun next(): ThrumClient.Event = events.poll(5, TimeUnit.SECONDS) ?: error("no event")

    private fun nextNot(heard: Boolean = true): ThrumClient.Event {
        while (true) {
            val e = next()
            if (!heard || e !is ThrumClient.Event.Heard) return e
        }
    }

    @Test
    fun theHelloIsSignedForThisBeeAndAdmitted() {
        FakeHumd().use { humd ->
            client(humd.dial)
            val hello = humd.nextHello()
            assertEquals(key.hid, hello.str("hid"))
            assertEquals(key.hid, hello.str("from"))
            assertEquals(hex(key.pubkey), hello.str("pubkey"))
            assertEquals(key.sign(hello.long("signedAt")!!), hello.str("signature"))
            assertTrue(System.currentTimeMillis() - hello.long("signedAt")!! < 5_000)
            assertEquals(THRUM_VERSION, hello.str("protoVersion"))
            assertEquals(52, hello.str("rid")!!.length)
            assertEquals(ThrumClient.Event.Nestled, next())
            val breath = (next() as ThrumClient.Event.Heard).tone
            assertEquals(Chi.BREATH, breath.str("chi"))
            assertTrue("the breath on connect admits nothing", breath["sigils"] is Tone)
        }
    }

    @Test
    fun aToneGoesToHumdOnlyWhileNestled() {
        FakeHumd().use { humd ->
            val c = client(humd.dial)
            assertEquals(ThrumClient.Event.Nestled, nextNot())
            assertTrue(c.send(tone(Chi.PROMPT, "rid" to "p1", "sid" to "s", "text" to "hi")))
            assertEquals("p1", humd.nextHeard().str("rid"))
            humd.say(tone(Chi.UNFURL, "sid" to "s", "petal" to 0, "kind" to "text", "text" to "yo"))
            var heard: Tone
            do heard = (next() as ThrumClient.Event.Heard).tone while (heard.str("chi") != Chi.UNFURL)
            assertEquals("yo", heard.str("text"))

            humd.drop()
            assertEquals(ThrumClient.Event.Astray, nextNot())
            assertEquals(ThrumClient.Event.Nestled, nextNot())
            assertEquals(2, humd.connections)
            assertEquals(2, humd.hellos.size)
            assertTrue(c.send(tone(Chi.CANCEL, "rid" to "p2", "sid" to "s")))
            assertEquals("p2", humd.nextHeard().str("rid"))
        }
    }

    @Test
    fun aRefusalIsFinal() {
        FakeHumd { "bee has not entered under this hive for this role" }.use { humd ->
            val c = client(humd.dial)
            val refused = nextNot() as ThrumClient.Event.Refused
            assertEquals("bee has not entered under this hive for this role", refused.why)
            assertFalse(c.send(tone(Chi.PROMPT, "sid" to "s", "text" to "x")))
            assertNull(events.poll(1, TimeUnit.SECONDS))
            assertEquals(1, humd.connections)
        }
    }

    @Test
    fun aHumdThatGivesNoVerdictIsDialedAgain() {
        val server = ServerSocket(0, 8, InetAddress.getLoopbackAddress())
        val accepted = LinkedBlockingQueue<Int>()
        thread(isDaemon = true) {
            var n = 0
            while (!server.isClosed) runCatching { server.accept().also { accepted += ++n } }
        }
        server.use {
            val c = client(dial(Humd.Tcp("127.0.0.1", server.localPort)), verdictMs = 200)
            assertEquals(1, accepted.poll(5, TimeUnit.SECONDS))
            assertFalse(c.send(tone(Chi.PROMPT, "sid" to "s", "text" to "x")))
            assertEquals(2, accepted.poll(5, TimeUnit.SECONDS))
            assertTrue(events.none { it == ThrumClient.Event.Nestled })
        }
    }

    @Test
    fun theBackoffDoublesToThirtySecondsWithAFifthOfJitter() {
        for ((tries, base) in listOf(0 to 250L, 1 to 500L, 3 to 2_000L, 7 to 30_000L, 40 to 30_000L)) {
            repeat(20) {
                val wait = ThrumClient.backoff(tries)
                assertTrue("$tries: $wait", wait in (base * 0.8).toLong()..(base * 1.2).toLong())
            }
        }
    }

    @Test
    fun anAddressIsASocketOrAHostAndPort() {
        assertEquals(Humd.Unix("/data/x/thrum.sock"), Humd.parse("/data/x/thrum.sock"))
        assertEquals(Humd.Tcp("127.0.0.1", 4378), Humd.parse("127.0.0.1:4378"))
        assertEquals(Humd.Tcp("::1", 4378), Humd.parse("[::1]:4378"))
        assertEquals(Humd.Tcp("localhost", 1), Humd.parse(" localhost:1 "))
        assertNull(Humd.parse("127.0.0.1"))
        assertNull(Humd.parse(":4378"))
        assertNull(Humd.parse("host:0"))
        assertNull(Humd.parse("host:http"))
    }
}
