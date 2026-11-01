// The stream: one tone per line, UTF-8, ended by \n, the same to humd and
// on a P2P link. A line longer than MAX_LINE bytes is dropped whole, as a
// stream between humds drops one, so no peer can make the bee hold more
// than that of a line; a last line the stream ends before its \n is
// dropped too.

package hum.hive.wifip2p

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream

const val MAX_LINE = 256 * 1024

class Lines(input: InputStream, private val max: Int = MAX_LINE) {
    private val input = BufferedInputStream(input)
    private val line = ByteArrayOutputStream()

    /** The next line that is not blank, without its \n; null once the stream ends. */
    fun next(): String? {
        var over = false
        while (true) {
            val b = input.read()
            if (b < 0) return null
            if (b == '\n'.code) {
                val text = line.toString(Charsets.UTF_8.name())
                line.reset()
                if (!over && text.isNotBlank()) return text
                over = false
            } else if (over || line.size() == max) {
                over = true
                line.reset()
            } else {
                line.write(b)
            }
        }
    }
}

fun line(tone: Tone): ByteArray = "$tone\n".toByteArray(Charsets.UTF_8)
