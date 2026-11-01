package hum.hive.wifip2p

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** Holds Tone.kt, and every tone the bee sounds, to hum.thrum. */
class ThrumTest {
    private class Thrum(val version: String, val envelope: Set<String>, val chis: Map<String, Set<String>>)

    private val thrum: Thrum by lazy {
        val file = File(System.getProperty("hum.thrum") ?: "../../../hum.thrum")
        var version = ""
        val envelope = mutableSetOf<String>()
        val chis = linkedMapOf<String, MutableSet<String>>()
        var fields: MutableSet<String>? = null
        for (raw in file.readLines()) {
            val words = raw.trim().split(Regex("\\s+"))
            when (words[0]) {
                "THRUM" -> version = words[1]
                "ENVELOPE" -> fields = envelope
                "CHI" -> fields = mutableSetOf<String>().also { chis[words[1]] = it }
                "SHAPE", "KIND", "PATTERN", "ADVISORY" -> fields = null
                "FIELD" -> fields?.add(words[1])
            }
        }
        Thrum(version, envelope, chis)
    }

    private fun assertDeclared(tone: Tone) {
        val chi = tone.str("chi")!!
        val declared = thrum.envelope + thrum.chis.getValue(chi)
        assertEquals("$chi carries keys hum.thrum does not declare for it", emptySet<String>(), tone.keys - declared)
    }

    @Test
    fun theVersionIsHumThrums() = assertEquals(thrum.version, THRUM_VERSION)

    @Test
    fun theRangeIsHumThrums() = assertEquals(thrum.chis.keys, Chi.RANGE)

    @Test
    fun theHelloCarriesOnlyDeclaredKeysAndEveryRequiredOne() {
        val sent = FakeHumd().use { humd ->
            val scope = CoroutineScope(Dispatchers.IO)
            ThrumClient(humd.dial, HIVE, Role.FORAGER, BeeKey(ByteArray(32)), HELLO) {}.start(scope)
            humd.nextHello().also { scope.cancel() }
        }
        assertDeclared(sent)
        for (required in listOf("chi", "rid", "protoVersion", "hive", "role", "hid", "pubkey", "signedAt", "signature")) {
            assertTrue("hello lacks $required", required in sent)
        }
        assertEquals(Role.FORAGER, sent.str("role"))
        assertEquals(HIVE, sent.str("hive"))
        for (chi in HELLO.toMap()["chis"] as List<*>) assertTrue(chi in Chi.RANGE)
    }

    @Test
    fun whatTheForagerSoundsIsDeclared() {
        val toHumd = mutableListOf<Tone>()
        val toPeer = mutableListOf<Tone>()
        val forage = Forage(Forage.Config(modelId = "m"), { toHumd += it; true }, { _, t -> toPeer += t })
        forage.heard("p", "h", """{"chi":"hello","rid":"r0"}""")
        forage.heard("p", "h", """{"chi":"prompt","rid":"r1","sid":"s","text":"hi","dusk":${Long.MAX_VALUE}}""")
        val sid = toHumd.last().str("sid")
        forage.hum(tone(Chi.UNFURL, "sid" to sid, "petal" to 0, "kind" to "text", "text" to "yo"))
        forage.hum(tone(Chi.WILT, "sid" to sid, "reason" to "stop"))
        forage.heard("p", "h", """{"chi":"prompt","rid":"r2","sid":"t","text":"hi"}""")
        forage.hum(tone(Chi.ERROR, "sid" to toHumd.last().str("sid"), "code" to "no-nurse", "message" to "none"))
        forage.heard("p", "h", """{"chi":"cancel","sid":"s"}""")
        forage.heard("p", "h", """{"chi":"cleanup","sid":"s"}""")
        assertEquals(listOf(Chi.PROMPT, Chi.PROMPT, Chi.CANCEL, Chi.CLEANUP), toHumd.map { it.str("chi") })
        assertEquals(listOf(Chi.BREATH, Chi.UNFURL, Chi.WILT, Chi.ERROR), toPeer.map { it.str("chi") })
        (toHumd + toPeer).forEach(::assertDeclared)
    }
}
