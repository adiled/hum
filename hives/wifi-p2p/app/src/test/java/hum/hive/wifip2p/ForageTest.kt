package hum.hive.wifip2p

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ForageTest {
    private val toHumd = mutableListOf<Tone>()
    private val toPeers = mutableListOf<Pair<String, Tone>>()
    private var away = false
    private var clock = 1_000L

    private fun forage(config: Forage.Config = Forage.Config()) =
        Forage(config, { if (away) false else { toHumd += it; true } }, { peer, t -> toPeers += peer to t }, { clock })

    private fun Forage.ask(peer: String, host: String, sid: String?, text: String, rid: String = "r-$text"): String {
        heard(peer, host, tone(Chi.PROMPT, "rid" to rid, "sid" to sid, "text" to text).toString())
        return toHumd.last().str("sid")!!
    }

    private fun Forage.grow(sid: String, vararg petals: String, reason: String = "stop") {
        petals.forEachIndexed { i, text -> hum(tone(Chi.UNFURL, "sid" to sid, "petal" to i, "kind" to "text", "text" to text)) }
        hum(tone(Chi.WILT, "sid" to sid, "reason" to reason))
    }

    @Test
    fun aPromptOpensABloomWithNoModelUnlessOneIsSaid() {
        val f = forage()
        f.ask("10.0.0.2:5000", "10.0.0.2", "s1", "hi")
        val prompt = toHumd.single()
        assertEquals(Chi.PROMPT, prompt.str("chi"))
        assertEquals("hi", prompt.str("text"))
        assertNull(prompt["modelId"])
        assertEquals(Forage.DEFAULT_SYSTEM, prompt.str("systemPrompt"))
        assertEquals(Forage.sidOn("10.0.0.2", "s1"), prompt.str("sid"))
        assertEquals(52, prompt.str("rid")!!.length)

        forage(Forage.Config(modelId = "claude-haiku-4-5", systemPrompt = null)).ask("p", "h", "s", "x")
        assertEquals("claude-haiku-4-5", toHumd.last().str("modelId"))
        assertNull(toHumd.last()["systemPrompt"])

        forage().heard("p", "h", """{"chi":"prompt","sid":"s","text":"x","modelId":"gpt-4o","systemPrompt":"be brief"}""")
        assertEquals("gpt-4o", toHumd.last().str("modelId"))
        assertEquals("be brief", toHumd.last().str("systemPrompt"))
    }

    @Test
    fun theBloomGoesBackToThePeerThatAskedAsOnePetalAndAWilt() {
        val f = forage()
        val a = f.ask("a:1", "a", "s", "one")
        f.ask("b:1", "b", "s", "two")
        f.grow(a, "  hello", " world  ")
        assertEquals(listOf("a:1", "a:1"), toPeers.map { it.first })
        val (petal, wilt) = toPeers.map { it.second }
        assertEquals("""{"chi":"unfurl","rid":"r-one","sid":"s","petal":0,"kind":"text","text":"hello world","closed":true}""", petal.toString())
        assertEquals("""{"chi":"wilt","rid":"r-one","sid":"s","reason":"stop"}""", wilt.toString())
    }

    @Test
    fun peersNeverShareALineOfBlooms() {
        val ids = setOf(Forage.sidOn("a", "s"), Forage.sidOn("b", "s"), Forage.sidOn("a", "t"), Forage.sidOn("a", null))
        assertEquals(4, ids.size)
        assertEquals(Forage.sidOn("a", "s"), Forage.sidOn("a", "s"))
        assertTrue(ids.all { it.startsWith("wifi-p2p-") })
    }

    @Test
    fun aBloomWithNoWordsAndALongOneAreSaidSo() {
        val f = forage(Forage.Config(replyLimit = 16))
        f.grow(f.ask("p", "h", "s", "quiet"))
        assertEquals("(no reply)", toPeers[0].second.str("text"))
        f.grow(f.ask("p", "h", "s", "loud"), "a".repeat(10), "b".repeat(10))
        assertEquals("a".repeat(10) + "bbb...", toPeers[2].second.str("text"))
        f.grow(f.ask("p", "h", "s", "exact"), "c".repeat(16))
        assertEquals("c".repeat(16), toPeers[4].second.str("text"))
    }

    @Test
    fun petalsThatAreNotTextAreLeftOut() {
        val f = forage()
        val sid = f.ask("p", "h", "s", "x")
        f.hum(tone(Chi.UNFURL, "sid" to sid, "petal" to 0, "kind" to "sepal", "text" to "thinking"))
        f.hum(tone(Chi.UNFURL, "sid" to sid, "petal" to 1, "kind" to "text", "text" to "said"))
        f.hum(tone(Chi.WILT, "sid" to sid, "reason" to "stop"))
        assertEquals("said", toPeers[0].second.str("text"))
    }

    @Test
    fun aBloomThatBreaksOffSaysWhy() {
        val f = forage()
        val sid = f.ask("p", "h", "s", "x")
        f.hum(tone(Chi.ERROR, "sid" to sid, "code" to "no-nurse", "message" to "no nurse is here"))
        assertEquals("""{"chi":"error","rid":"r-x","sid":"s","code":"no-nurse","message":"no nurse is here"}""", toPeers.single().second.toString())
        f.hum(tone(Chi.WILT, "sid" to sid, "reason" to "stop"))
        assertEquals(1, toPeers.size)
    }

    @Test
    fun aPromptHumdRefusesIsBrokenOff() {
        val f = forage()
        f.ask("p", "h", "s", "x")
        val rid = toHumd.last().str("rid")
        f.hum(tone(Chi.ECHO, "rid" to rid, "ok" to true))
        assertTrue(toPeers.isEmpty())
        f.hum(tone(Chi.ECHO, "rid" to rid, "ok" to false, "error" to "past dusk"))
        assertEquals("refused", toPeers.single().second.str("code"))
        assertEquals("past dusk", toPeers.single().second.str("message"))
    }

    @Test
    fun whileHumdIsAwayAPromptIsTurnedBack() {
        val f = forage()
        away = true
        f.heard("p", "h", """{"chi":"prompt","rid":"r","text":"x"}""")
        assertEquals("away", toPeers.single().second.str("code"))
        assertNull(toPeers.single().second["sid"])
        away = false
        f.ask("p", "h", null, "x")
        assertEquals(1, toHumd.size)
    }

    @Test
    fun aSecondPromptOnAGrowingSidIsTurnedBack() {
        val f = forage()
        f.ask("p", "h", "s", "x")
        f.heard("p", "h", """{"chi":"prompt","rid":"r2","sid":"s","text":"y"}""")
        assertEquals(1, toHumd.size)
        assertEquals("growing", toPeers.single().second.str("code"))
        assertEquals("r2", toPeers.single().second.str("rid"))
    }

    @Test
    fun humdGoingAwayBreaksOffEveryBloom() {
        val f = forage()
        f.ask("a:1", "a", "s", "x")
        f.ask("b:1", "b", "s", "y")
        f.astray()
        assertEquals(setOf("a:1", "b:1"), toPeers.map { it.first }.toSet())
        assertTrue(toPeers.all { it.second.str("code") == "astray" })
    }

    @Test
    fun aPeerThatLeavesCancelsOnlyItsOwnBlooms() {
        val f = forage()
        val a = f.ask("a:1", "a", "s", "x")
        val b = f.ask("b:1", "b", "s", "y")
        f.gone("a:1")
        assertEquals(tone(Chi.CANCEL, "rid" to toHumd.last().str("rid"), "sid" to a), toHumd.last())
        f.grow(a, "late")
        f.grow(b, "kept")
        assertEquals(listOf("b:1", "b:1"), toPeers.map { it.first })
    }

    @Test
    fun cancelAndCleanupGoToHumdUnderThePeersOwnSid() {
        val f = forage()
        f.heard("p", "h", """{"chi":"cancel","sid":"s"}""")
        f.heard("p", "h", """{"chi":"cleanup","sid":"s"}""")
        assertEquals(listOf(Chi.CANCEL, Chi.CLEANUP), toHumd.map { it.str("chi") })
        assertTrue(toHumd.all { it.str("sid") == Forage.sidOn("h", "s") })
    }

    @Test
    fun aHelloIsAnsweredWithWhatThisHumdCanReach() {
        val f = forage()
        f.heard("p", "h", """{"chi":"hello","rid":"h1"}""")
        assertEquals("""{"chi":"breath","rid":"h1","sigils":{}}""", toPeers.last().second.toString())
        f.hum(parseTone("""{"chi":"breath","sigils":{},"models":[{"id":"claude-haiku-4-5","hive":"claude-cli"}]}""")!!)
        f.heard("p", "h", """{"chi":"hello","rid":"h2"}""")
        assertEquals("""{"chi":"breath","rid":"h2","sigils":{},"models":[{"id":"claude-haiku-4-5","hive":"claude-cli"}]}""", toPeers.last().second.toString())
    }

    @Test
    fun aToneThatIsPastDuskOrNoToneIsDropped() {
        val f = forage()
        clock = 2_000
        f.heard("p", "h", """{"chi":"prompt","text":"late","dusk":1999}""")
        f.heard("p", "h", "garbage")
        f.heard("p", "h", """{"chi":"echo","rid":"x","ok":true}""")
        assertTrue(toHumd.isEmpty() && toPeers.isEmpty())
        f.heard("p", "h", """{"chi":"prompt","text":"soon","dusk":3000}""")
        assertEquals(3000L, toHumd.single().long("dusk"))
    }

    @Test
    fun aReplyWithoutASidOrRidGetsOneRidAndNoSid() {
        val f = forage()
        f.heard("p", "h", """{"chi":"prompt","text":"x"}""")
        f.grow(toHumd.last().str("sid")!!, "y")
        val (petal, wilt) = toPeers.map { it.second }
        assertNull(petal["sid"])
        assertEquals(petal.str("rid"), wilt.str("rid"))
        assertNotEquals(null, petal.str("rid"))
    }
}
