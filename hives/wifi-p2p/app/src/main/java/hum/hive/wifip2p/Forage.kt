// What the forager does: a prompt from a phone in the group opens a bloom
// on this bee's humd, and the bloom goes back to that phone alone. A peer
// is one link, known by its <ip>:<port>. Each sid a peer names is a line
// of blooms of its own on humd, drawn from the peer's address and that
// sid, so no peer grows on another's; a peer that names none has one line.
// Lean: a bloom's text petals are gathered, and go back whole when it
// wilts. A peer that leaves cancels the blooms still growing for it, and
// humd going away breaks off every one.

package hum.hive.wifip2p

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

class Forage(
    private val config: Config,
    private val humd: (Tone) -> Boolean,
    private val peer: (String, Tone) -> Unit,
    private val now: () -> Long = System::currentTimeMillis,
) {
    data class Config(
        val modelId: String? = null,
        val systemPrompt: String? = DEFAULT_SYSTEM,
        val replyLimit: Int = 4096,
    )

    private class Bloom(val peer: String, val rid: String, val sid: String?, val prompt: String) {
        val text = StringBuilder()
    }

    private val blooms = HashMap<String, Bloom>()
    private var models: JsonElement? = null

    @Synchronized
    fun heard(from: String, host: String, line: String) {
        val tone = parseTone(line) ?: return
        val chi = tone.str("chi") ?: return
        val dusk = tone.long("dusk")
        if (dusk != null && now() > dusk) return
        val sid = sidOn(host, tone.str("sid"))
        when (chi) {
            Chi.PROMPT -> prompt(from, sid, tone)
            Chi.CANCEL, Chi.CLEANUP -> humd(tone(chi, "rid" to rid(), "sid" to sid))
            Chi.HELLO -> peer(
                from,
                tone(Chi.BREATH, "rid" to (tone.str("rid") ?: rid()), "sigils" to JsonObject(emptyMap()), "models" to models),
            )
        }
    }

    @Synchronized
    fun hum(tone: Tone) {
        when (tone.str("chi")) {
            Chi.BREATH -> tone["models"]?.let { models = it }
            Chi.ECHO -> if (tone.bool("ok") == false) {
                val sid = blooms.entries.firstOrNull { it.value.prompt == tone.str("rid") }?.key ?: return
                breakOff(sid, "refused", tone.str("error") ?: "humd refused the prompt")
            }
            Chi.UNFURL -> {
                val bloom = blooms[tone.str("sid")] ?: return
                val text = tone.str("text")
                if (tone.str("kind") == PetalKind.TEXT && text != null) {
                    bloom.text.append(text.take(config.replyLimit + 1 - bloom.text.length))
                }
            }
            Chi.WILT -> {
                val bloom = blooms.remove(tone.str("sid")) ?: return
                var reply = bloom.text.toString().trim().ifEmpty { "(no reply)" }
                if (bloom.text.length > config.replyLimit) reply = reply.take(config.replyLimit - 3) + "..."
                peer(
                    bloom.peer,
                    tone(Chi.UNFURL, "rid" to bloom.rid, "sid" to bloom.sid, "petal" to 0, "kind" to PetalKind.TEXT, "text" to reply, "closed" to true),
                )
                peer(bloom.peer, tone(Chi.WILT, "rid" to bloom.rid, "sid" to bloom.sid, "reason" to (tone.str("reason") ?: "stop")))
            }
            Chi.ERROR -> {
                val sid = tone.str("sid") ?: return
                breakOff(sid, tone.str("code"), tone.str("message") ?: "the bloom broke off")
            }
        }
    }

    @Synchronized
    fun gone(from: String) {
        val left = blooms.filterValues { it.peer == from }.keys
        for (sid in left) {
            blooms.remove(sid)
            humd(tone(Chi.CANCEL, "rid" to rid(), "sid" to sid))
        }
    }

    @Synchronized
    fun astray() {
        for (sid in blooms.keys.toList()) breakOff(sid, "astray", "humd went away mid-bloom")
    }

    private fun prompt(from: String, sid: String, asked: Tone) {
        val replyRid = asked.str("rid") ?: rid()
        val replySid = asked.str("sid")
        fun refuse(code: String, message: String) =
            peer(from, tone(Chi.ERROR, "rid" to replyRid, "sid" to replySid, "code" to code, "message" to message))
        if (sid in blooms) return refuse("growing", "a bloom is already open on this sid")
        val promptRid = rid()
        val prompt = tone(
            Chi.PROMPT,
            "rid" to promptRid,
            "sid" to sid,
            "dusk" to asked.long("dusk"),
            "modelId" to (asked.str("modelId") ?: config.modelId)?.takeIf { it.isNotBlank() },
            "systemPrompt" to (asked.str("systemPrompt") ?: config.systemPrompt)?.takeIf { it.isNotBlank() },
            "text" to asked.str("text"),
        )
        blooms[sid] = Bloom(from, replyRid, replySid, promptRid)
        if (!humd(prompt)) {
            blooms.remove(sid)
            refuse("away", "humd is away")
        }
    }

    private fun breakOff(sid: String, code: String?, message: String) {
        val bloom = blooms.remove(sid) ?: return
        peer(bloom.peer, tone(Chi.ERROR, "rid" to bloom.rid, "sid" to bloom.sid, "code" to code, "message" to message))
    }

    companion object {
        const val DEFAULT_SYSTEM = "You are a concise assistant for P2P mesh agents."

        fun sidOn(host: String, sid: String?): String =
            "wifi-p2p-" + hex(sha256("$host ${sid.orEmpty()}".toByteArray(Charsets.UTF_8))).take(16)
    }
}
