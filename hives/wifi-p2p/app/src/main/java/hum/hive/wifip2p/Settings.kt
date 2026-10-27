// The bee's settings: hum/wifi-p2p.json in the app's files directory, put
// there beside the key. A key left out, or a file that does not parse,
// takes the default.

package hum.hive.wifip2p

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import java.io.File

data class Settings(
    val humd: String = "127.0.0.1:4378",
    val p2p: P2pManager.Config = P2pManager.Config(),
    val forage: Forage.Config = Forage.Config(),
) {
    companion object {
        fun parse(text: String): Settings {
            val json = runCatching { Json.parseToJsonElement(text) }.getOrNull() as? JsonObject ?: return Settings()
            val p2p = P2pManager.Config()
            val forage = Forage.Config()
            return Settings(
                humd = json.str("humd") ?: Settings().humd,
                p2p = P2pManager.Config(
                    port = json.long("port")?.toInt()?.takeIf { it in 1..65535 } ?: p2p.port,
                    goIntent = json.long("goIntent")?.toInt()?.takeIf { it in 0..15 } ?: p2p.goIntent,
                    serviceType = json.str("serviceType") ?: p2p.serviceType,
                ),
                forage = Forage.Config(
                    modelId = json.str("modelId") ?: forage.modelId,
                    systemPrompt = if (json.containsKey("systemPrompt")) json.str("systemPrompt") else forage.systemPrompt,
                    replyLimit = json.long("replyLimit")?.toInt()?.takeIf { it >= 16 } ?: forage.replyLimit,
                ),
            )
        }

        fun load(file: File): Settings = if (file.isFile) parse(file.readText()) else Settings()
    }
}
