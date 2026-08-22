package hum.hive.wifip2p

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.security.MessageDigest
import java.time.Instant

// ── Chi registry ─────────────────────────────────────────────
// Mirrors thrum-core/src/chi.rs (the Rust source of truth).
// Wire values are kebab-case.

const val THRUM_VERSION = "0.7.0"
const val HIVE_NAME = "wifi-p2p"

object Chi {
    const val HELLO = "hello"
    const val PROMPT = "prompt"
    const val CANCEL = "cancel"
    const val CLEANUP = "cleanup"
    const val CURATE = "curate"
    const val RELEASE_PERMIT = "release-permit"
    const val TENDRIl_RESULT = "tendril-result"
    const val TOOL_RESULT = "tool-result"
    const val PETAL_CELL = "petal-cell"
    const val BREATH = "breath"
    const val CHUNK = "chunk"
    const val FINISH = "finish"
    const val ERROR = "error"
    const val SESSION_READY = "session-ready"
    const val PULSE = "pulse"
    const val PERMISSION_ASK = "permission-ask"
    const val TENDRIl_REACH = "tendril-reach"
    const val TOOL_CALL = "tool-call"
    const val TOOL_META = "tool-meta"
    const val TOOL_INFO = "tool-info"
    const val ECHO = "echo"
    const val PERF_MARK = "perf-mark"
    const val LOG = "log"
    const val DRONE = "drone"
    const val DRONE_RETROFIT = "drone-retrofit"
}

// ── Tone envelope ────────────────────────────────────────────

data class Tone(
    val chi: String,
    val rid: String,
    val sid: String? = null,
    val from: String? = null,
    val to: String? = null,
    val sigil: String? = null,
    val wane: Int? = null,
    val sentAt: Long? = null,
    val dusk: Long? = null,
    val ext: Map<String, Map<String, Any>>? = null,
    val body: Map<String, Any> = emptyMap(),
) {
    fun toJson(): String {
        val sb = StringBuilder("{")
        appendField(sb, "chi", chi)
        sb.append(',')
        appendField(sb, "rid", rid)
        sid?.let { sb.append(','); appendField(sb, "sid", it) }
        from?.let { sb.append(','); appendField(sb, "from", it) }
        to?.let { sb.append(','); appendField(sb, "to", it) }
        sigil?.let { sb.append(','); appendField(sb, "sigil", it) }
        wane?.let { sb.append(','); appendField(sb, "wane", it.toLong()) }
        sentAt?.let { sb.append(','); appendField(sb, "sentAt", it) }
        dusk?.let { sb.append(','); appendField(sb, "dusk", it) }
        ext?.let { sb.append(','); appendField(sb, "ext", it) }
        for ((k, v) in body) {
            sb.append(',')
            appendFieldRaw(sb, k, v)
        }
        sb.append('}')
        return sb.toString()
    }

    companion object {
        fun fromJson(json: String): Tone {
            val obj = parseJsonObject(json)
            val chi = obj["chi"] as? String ?: error("missing chi")
            val rid = obj["rid"] as? String ?: error("missing rid")
            val sid = obj["sid"] as? String?
            val from = obj["from"] as? String?
            val to = obj["to"] as? String?
            val sigil = obj["sigil"] as? String?
            val wane = (obj["wane"] as? Number)?.toInt()
            val sentAt = (obj["sentAt"] as? Number)?.toLong()
            val dusk = (obj["dusk"] as? Number)?.toLong()
            val ext = obj["ext"] as? Map<String, Map<String, Any>>?
            val body = obj.filterKeys { it !in envelopeKeys }
            return Tone(chi, rid, sid, from, to, sigil, wane, sentAt, dusk, ext, body)
        }

        private val envelopeKeys = setOf("chi", "rid", "sid", "from", "to", "sigil", "wane", "sentAt", "dusk", "ext")
        private fun parseJsonObject(json: String): Map<String, Any> {
            // Minimal JSON parser for NDJSON tones — no dependency needed.
            // In production, use kotlinx-serialization.
            val trimmed = json.trim().removeSurrounding("{", "}")
            val result = mutableMapOf<String, Any>()
            var depth = 0
            var key: String? = null
            var inKey = false
            var inVal = false
            var valStr = StringBuilder()
            var keyStr = StringBuilder()
            var quote: Char? = null
            for (c in trimmed) {
                if (quote != null) {
                    if (c == '\\') { /* skip escape handling — simplified */ }
                    else if (c == quote) { quote = null }
                    else { if (inKey) keyStr.append(c) else valStr.append(c) }
                    continue
                }
                when {
                    c == '"' -> { quote = c; if (inKey) Unit else inVal = true }
                    c == ':' && inKey -> { inKey = false; inVal = true; key = keyStr.toString().trim().removeSurrounding("\""); keyStr.clear() }
                    c == ',' && depth == 0 -> {
                        inVal = false
                        valStr.clear()
                        inKey = true
                    }
                    c == '{' -> depth++
                    c == '}' -> depth--
                    inKey && !inVal -> keyStr.append(c)
                    inVal && depth > 0 -> valStr.append(c)
                }
            }
            return result
        }
    }
}

private fun appendField(sb: StringBuilder, key: String, value: Any?) {
    when (value) {
        is String -> sb.append('"').append(key).append('"').append(':').append('"').append(escapeJson(value)).append('"')
        is Number -> sb.append('"').append(key).append('"').append(':').append(value)
        is Boolean -> sb.append('"').append(key).append('"').append(':').append(value)
        is Map<*, *> -> {
            sb.append('"').append(key).append('"').append(':')
            val entries = value.entries.joinToString(",") { (k, v) ->
                "\"${escapeJson(k.toString())}\":${jsonValue(v)}"
            }
            sb.append('{').append(entries).append('}')
        }
        null -> sb.append('"').append(key).append('"').append(':').append("null")
        else -> sb.append('"').append(key).append('"').append(':').append('"').append(escapeJson(value.toString())).append('"')
    }
}

private fun appendFieldRaw(sb: StringBuilder, key: String, value: Any?) {
    when (value) {
        is String -> sb.append('"').append(key).append('"').append(':').append('"').append(escapeJson(value)).append('"')
        is Number -> sb.append('"').append(key).append('"').append(':').append(value)
        is Boolean -> sb.append('"').append(key).append('"').append(':').append(value)
        is Map<*, *> -> {
            sb.append('"').append(key).append('"').append(':')
            val entries = value.entries.joinToString(",") { (k, v) ->
                "\"${escapeJson(k.toString())}\":${jsonValue(v)}"
            }
            sb.append('{').append(entries).append('}')
        }
        is List<*> -> {
            sb.append('"').append(key).append('"').append(':')
            val items = value.joinToString(",") { jsonValue(it) }
            sb.append('[').append(items).append(']')
        }
        null -> sb.append('"').append(key).append('"').append(':').append("null")
        else -> sb.append('"').append(key).append('"').append(':').append('"').append(escapeJson(value.toString())).append('"')
    }
}

private fun jsonValue(v: Any?): String = when (v) {
    null -> "null"
    is String -> "\"${escapeJson(v)}\""
    is Number -> v.toString()
    is Boolean -> v.toString()
    is Map<*, *> -> {
        val entries = v.entries.joinToString(",") { (k, val) ->
            "\"${escapeJson(k.toString())}\":${jsonValue(val)}"
        }
        "{$entries}"
    }
    is List<*> -> {
        val items = v.joinToString(",") { jsonValue(it) }
        "[$items]"
    }
    else -> "\"${escapeJson(v.toString())}\""
}

private fun escapeJson(s: String): String = s
    .replace("\\", "\\\\")
    .replace("\"", "\\\"")
    .replace("\n", "\\n")
    .replace("\r", "\\r")
    .replace("\t", "\\t")

// ── Helpers (mirror thrum-core) ───────────────────────────────

fun sigil(sid: String, nest: String): String {
    val digest = MessageDigest.getInstance("SHA-256")
    digest.update("$nest:$sid".toByteArray())
    return digest.digest().take(6).joinToString("") { "%02x".format(it) }
}

private var ridCounter = 0L
fun rid(): String {
    val ts = System.currentTimeMillis().toString(36)
    val c = (ridCounter++).toString(36)
    return "$ts-$c"
}

fun duskIn(ms: Long): Long = System.currentTimeMillis() + ms
