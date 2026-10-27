package hum.hive.wifip2p

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ToneTest {
    @Test
    fun aToneIsFlatAndAnAbsentFieldIsAbsent() {
        val t = tone(Chi.PROMPT, "rid" to "r", "sid" to null, "text" to "hi", "season" to 64, "closed" to true)
        assertEquals("""{"chi":"prompt","rid":"r","text":"hi","season":64,"closed":true}""", t.toString())
    }

    @Test
    fun nestedMapsAndListsEncode() {
        val t = tone(Chi.HELLO, "propensity" to mapOf("a" to "b", "none" to null), "chis" to listOf("hello", null, "prompt"))
        assertEquals("""{"chi":"hello","propensity":{"a":"b"},"chis":["hello","prompt"]}""", t.toString())
    }

    @Test
    fun textIsEscapedSoATonesStaysOneLine() {
        val t = tone(Chi.UNFURL, "text" to "a \"quote\"\nnew line\t\\ é ✓")
        val encoded = t.toString()
        assertFalse(encoded.contains('\n'))
        assertEquals("a \"quote\"\nnew line\t\\ é ✓", parseTone(encoded)!!.str("text"))
    }

    @Test
    fun aLineThatIsNoToneParsesToNothing() {
        assertNull(parseTone("not json"))
        assertNull(parseTone("[1,2]"))
        assertNull(parseTone("""{"rid":"r"}"""))
        assertNull(parseTone("""{"chi":""}"""))
        assertNull(parseTone("""{"chi":7}"""))
        assertNull(parseTone("""{"chi":"wilt""""))
    }

    @Test
    fun fieldsReadByTheirType() {
        val t = parseTone("""{"chi":"echo","rid":"r","ok":false,"dusk":1700000000000,"n":"12","s":3}""")!!
        assertEquals("echo", t.str("chi"))
        assertEquals(false, t.bool("ok"))
        assertEquals(1_700_000_000_000L, t.long("dusk"))
        assertNull(t.long("n"))
        assertNull(t.str("s"))
        assertNull(t.str("absent"))
    }

    @Test
    fun aToneCarriesUnknownFieldsWhole() {
        val line = """{"chi":"wilt","sid":"s","reason":"stop","nectar":{"in":3,"out":[1,2]},"later":{"x":null}}"""
        assertEquals(line, parseTone(line).toString())
    }

    @Test
    fun ridsAreHumIdsAsThrumCoreMintsThem() {
        assertEquals("0".repeat(52), humId(ByteArray(32)))
        assertEquals("Z".repeat(51) + "G", humId(ByteArray(32) { 0xff.toByte() }))
        val atOnly = ByteArray(32)
        val ms = 1_700_000_000_000L
        for (i in 0 until 6) atOnly[i] = (ms ushr (40 - 8 * i)).toByte()
        assertEquals("065WZSB8" + "0".repeat(44), humId(atOnly))

        val a = rid(ms)
        val b = rid(ms)
        assertTrue(a != b)
        assertTrue(Regex("^[0-9A-HJKMNP-TV-Z]{52}$").matches(a))
        assertEquals("065WZSB8", a.take(8))
    }
}
