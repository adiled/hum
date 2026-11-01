// The thrum as this bee speaks it. hum.thrum is the thrum, and no client
// is generated for Kotlin, so THRUM_VERSION and the range below are read
// out of it by hand and held to it by ThrumTest. A tone is a flat JSON
// object; an absent optional field is absent from the wire, not null.

package hum.hive.wifip2p

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.longOrNull
import java.security.SecureRandom

const val THRUM_VERSION = "0.8.0"

typealias Tone = JsonObject

object Chi {
    const val HELLO = "hello"
    const val PROMPT = "prompt"
    const val CANCEL = "cancel"
    const val CLEANUP = "cleanup"
    const val CURATE = "curate"
    const val SCOUT_RESULT = "scout-result"
    const val TENDRIL_RESULT = "tendril-result"
    const val BREATH = "breath"
    const val UNFURL = "unfurl"
    const val WILT = "wilt"
    const val ERROR = "error"
    const val PULSE = "pulse"
    const val SCOUT = "scout"
    const val TENDRIL_REACH = "tendril-reach"
    const val TENDRIL_META = "tendril-meta"
    const val TENDRIL_INFO = "tendril-info"
    const val ECHO = "echo"
    const val PERF_MARK = "perf-mark"
    const val LOG = "log"
    const val DRONE = "drone"
    const val DRONE_RETROFIT = "drone-retrofit"
    const val PEER_ADD = "peer-add"
    const val PEER_REMOVE = "peer-remove"
    const val ATTACH = "attach"
    const val DETACH = "detach"
    const val WANE_SYNC = "wane-sync"
    const val GOSSIP_PUBLISH = "gossip-publish"
    const val KAD_FIND_NODE = "kad-find-node"
    const val KAD_FIND_NODE_RESP = "kad-find-node-resp"
    const val BACKFILL = "backfill"
    const val BACKFILL_EVENT = "backfill-event"

    val RANGE = setOf(
        HELLO, PROMPT, CANCEL, CLEANUP, CURATE, SCOUT_RESULT, TENDRIL_RESULT,
        BREATH, UNFURL, WILT, ERROR, PULSE, SCOUT, TENDRIL_REACH, TENDRIL_META,
        TENDRIL_INFO, ECHO, PERF_MARK, LOG, DRONE, DRONE_RETROFIT, PEER_ADD,
        PEER_REMOVE, ATTACH, DETACH, WANE_SYNC, GOSSIP_PUBLISH, KAD_FIND_NODE,
        KAD_FIND_NODE_RESP, BACKFILL, BACKFILL_EVENT,
    )
}

object Role {
    const val FORAGER = "forager"
}

object PetalKind {
    const val TEXT = "text"
}

fun tone(chi: String, vararg fields: Pair<String, Any?>): Tone {
    val body = LinkedHashMap<String, JsonElement>()
    body["chi"] = JsonPrimitive(chi)
    for ((key, value) in fields) json(value)?.let { body[key] = it }
    return JsonObject(body)
}

private fun json(value: Any?): JsonElement? = when (value) {
    null, JsonNull -> null
    is JsonElement -> value
    is String -> JsonPrimitive(value)
    is Number -> JsonPrimitive(value)
    is Boolean -> JsonPrimitive(value)
    is Map<*, *> -> JsonObject(
        value.entries.mapNotNull { (k, v) -> json(v)?.let { k.toString() to it } }.toMap(LinkedHashMap()),
    )
    is Iterable<*> -> JsonArray(value.mapNotNull(::json))
    else -> throw IllegalArgumentException("${value::class.simpleName} is not JSON")
}

fun parseTone(line: String): Tone? {
    val tone = runCatching { Json.parseToJsonElement(line) }.getOrNull() as? JsonObject ?: return null
    return tone.takeIf { !it.str("chi").isNullOrEmpty() }
}

fun Tone.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content

fun Tone.long(key: String): Long? = (this[key] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull

fun Tone.bool(key: String): Boolean? = (this[key] as? JsonPrimitive)?.takeUnless { it.isString }?.booleanOrNull

private const val CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
private val random = SecureRandom()

/** A rid as thrum-core mints one: 48 bits of ms and 208 random, Crockford base32. */
fun rid(nowMs: Long = System.currentTimeMillis()): String {
    val id = ByteArray(32)
    random.nextBytes(id)
    for (i in 0 until 6) id[i] = (nowMs ushr (40 - 8 * i)).toByte()
    return humId(id)
}

internal fun humId(id: ByteArray): String = buildString(52) {
    for (char in 0 until 52) {
        var digit = 0
        for (b in char * 5 until char * 5 + 5) {
            val set = b < 256 && ((id[b / 8].toInt() shr (7 - b % 8)) and 1) == 1
            digit = (digit shl 1) or if (set) 1 else 0
        }
        append(CROCKFORD[digit])
    }
}
