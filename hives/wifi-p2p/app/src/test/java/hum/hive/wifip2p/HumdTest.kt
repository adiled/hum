package hum.hive.wifip2p

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The bee against a real humd: `./gradlew testDebugUnitTest -Phumd=<address>
 * -Pbee=<dir>`, where <dir> holds bee and bees/<id>.key as the phone would,
 * and -Pexpect=<text> is what the bloom should come back saying. Skipped
 * without them.
 */
class HumdTest {
    @Test
    fun theBeeNestlesAndItsPromptIsAnswered() {
        val address = System.getProperty("hum.humd")
        val dir = System.getProperty("hum.bee")
        assumeTrue("needs -Phumd and -Pbee", address != null && dir != null)
        val scope = CoroutineScope(Dispatchers.IO)
        val events = LinkedBlockingQueue<Any>()
        lateinit var client: ThrumClient
        val forage = Forage(Forage.Config(systemPrompt = null), { client.send(it) }, { _, t -> events += t })
        client = ThrumClient(dial(Humd.parse(address!!)!!), HIVE, Role.FORAGER, IdentityStore.load(File(dir!!)), HELLO) {
            if (it is ThrumClient.Event.Heard) forage.hum(it.tone) else events += it
        }
        try {
            client.start(scope)
            val verdict = events.poll(15, TimeUnit.SECONDS)
            println("humd: $verdict")
            assertEquals(ThrumClient.Event.Nestled, verdict)
            forage.heard("192.168.49.2:40000", "192.168.49.2", """{"chi":"prompt","rid":"ask-1","sid":"meetup","text":"one cup of chaos agent"}""")
            val reply = events.poll(30, TimeUnit.SECONDS) as Tone
            println("peer: $reply")
            System.getProperty("hum.expect")?.let { assertEquals(it, reply.str("text")) }
            if (reply.str("chi") == Chi.UNFURL) {
                val wilt = events.poll(5, TimeUnit.SECONDS) as Tone
                println("peer: $wilt")
                assertEquals(Chi.WILT, wilt.str("chi"))
            } else {
                assertTrue(reply.str("chi") == Chi.ERROR)
            }
        } finally {
            client.stop()
            scope.cancel()
        }
    }
}
