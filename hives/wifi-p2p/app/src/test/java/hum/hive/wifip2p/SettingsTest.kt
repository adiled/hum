package hum.hive.wifip2p

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SettingsTest {
    @Test
    fun noFileOrABadOneIsTheDefaults() {
        assertEquals(Settings(), Settings.parse(""))
        assertEquals(Settings(), Settings.parse("{not json"))
        assertEquals("127.0.0.1:4378", Settings().humd)
        assertNull(Settings().forage.modelId)
    }

    @Test
    fun eachKeySetsItsOwn() {
        val s = Settings.parse(
            """{"humd":"10.0.0.1:9000","port":4400,"goIntent":15,"modelId":"claude-haiku-4-5","systemPrompt":null,"replyLimit":100}""",
        )
        assertEquals("10.0.0.1:9000", s.humd)
        assertEquals(4400, s.p2p.port)
        assertEquals(15, s.p2p.goIntent)
        assertEquals("_hum._tcp", s.p2p.serviceType)
        assertEquals("claude-haiku-4-5", s.forage.modelId)
        assertNull(s.forage.systemPrompt)
        assertEquals(100, s.forage.replyLimit)
    }

    @Test
    fun aValueOutOfRangeIsTheDefault() {
        val s = Settings.parse("""{"port":70000,"goIntent":16,"replyLimit":2}""")
        assertEquals(Settings(), s)
    }
}
