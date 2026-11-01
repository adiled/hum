package hum.hive.wifip2p

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream

class StreamTest {
    private fun lines(bytes: ByteArray, max: Int = MAX_LINE, chunk: Int = Int.MAX_VALUE): List<String> {
        val input = object : InputStream() {
            val all = ByteArrayInputStream(bytes)
            override fun read() = all.read()
            override fun read(b: ByteArray, off: Int, len: Int) = all.read(b, off, minOf(len, chunk))
        }
        val lines = Lines(input, max)
        return generateSequence { lines.next() }.toList()
    }

    @Test
    fun oneToneALineAndBlankLinesAreNothing() {
        val got = lines("{\"chi\":\"a\"}\n\n   \n{\"chi\":\"b\"}\n".toByteArray())
        assertEquals(listOf("{\"chi\":\"a\"}", "{\"chi\":\"b\"}"), got)
    }

    @Test
    fun aLineArrivingInPiecesIsOneLine() {
        val text = "{\"chi\":\"unfurl\",\"text\":\"żółw ✓ 🐝\"}\n{\"chi\":\"wilt\"}\n"
        assertEquals(listOf("{\"chi\":\"unfurl\",\"text\":\"żółw ✓ 🐝\"}", "{\"chi\":\"wilt\"}"), lines(text.toByteArray(), chunk = 1))
    }

    @Test
    fun aLineTooLongIsDroppedWholeAndTheNextIsRead() {
        val long = "x".repeat(100)
        val got = lines("$long\nshort\n${"y".repeat(16)}\n".toByteArray(), max = 16)
        assertEquals(listOf("short", "y".repeat(16)), got)
    }

    @Test
    fun aLastLineWithoutItsNewlineIsDropped() {
        assertEquals(listOf("one"), lines("one\ntwo".toByteArray()))
        assertNull(Lines(ByteArrayInputStream(ByteArray(0))).next())
    }

    @Test
    fun aToneGoesOutAsOneLine() {
        val out = line(tone(Chi.WILT, "sid" to "s", "reason" to "stop\nnow"))
        assertArrayEquals("{\"chi\":\"wilt\",\"sid\":\"s\",\"reason\":\"stop\\nnow\"}\n".toByteArray(), out)
        assertEquals(1, out.count { it == '\n'.code.toByte() })
    }
}
