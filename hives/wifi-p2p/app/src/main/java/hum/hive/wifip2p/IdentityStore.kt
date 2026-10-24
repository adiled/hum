// The bee is entered on the humd it lands on (`hum bee enter --hive
// wifi-p2p`); the id that prints and the key filed under it are put in
// the app's data dir as `hum/bee` and `hum/bees/<id>.key`. Same seed
// format and signed message as hum-identity.

package hum.hive.wifip2p

import java.io.File
import java.security.MessageDigest
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer

object IdentityStore {
    private const val SOCKET_DOMAIN = "hum-bee-socket-v1"

    fun signedAs(dataDir: String): Map<String, Any> {
        val id = File(dataDir, "hum/bee").readText().trim()
        require(Regex("^bee_[0-9a-f]{64}$").matches(id)) { "hum/bee names no bee: $id" }
        val seed = File(dataDir, "hum/bees/$id.key").readBytes()
        require(seed.size == 32) { "bee key for $id is ${seed.size} bytes, expected 32" }

        val priv = Ed25519PrivateKeyParameters(seed, 0)
        val pub = priv.generatePublicKey().encoded
        val hid = "bee_" + hex(MessageDigest.getInstance("SHA-256").digest(pub))
        require(hid == id) { "the key filed as $id is not that bee's key" }

        val signedAt = System.currentTimeMillis()
        val signer = Ed25519Signer()
        signer.init(true, priv)
        val msg = "$SOCKET_DOMAIN:$hid:$signedAt".toByteArray()
        signer.update(msg, 0, msg.size)
        return mapOf(
            "hid" to hid,
            "pubkey" to hex(pub),
            "signedAt" to signedAt,
            "signature" to hex(signer.generateSignature()),
        )
    }

    private fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }
}
