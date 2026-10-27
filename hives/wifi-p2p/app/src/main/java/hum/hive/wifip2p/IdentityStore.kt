// A bee is its key. `hum bee enter --hive wifi-p2p --role forager --as
// <label>`, run on the humd the bee lands on, files the key and prints the
// bee's id; both go in the app's files directory, as hum/bee and
// hum/bees/<id>.key, the key the 32-byte seed hum filed. The hello is
// signed as hum-identity signs it: ed25519 over
// hum-bee-socket-v1:<hid>:<signedAt>.

package hum.hive.wifip2p

import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import java.io.File
import java.security.MessageDigest

class BeeKey(seed: ByteArray) {
    private val signing = Ed25519PrivateKeyParameters(seed.also { require(it.size == 32) }, 0)
    val pubkey: ByteArray = signing.generatePublicKey().encoded
    val hid: String = "bee_" + hex(sha256(pubkey))

    fun sign(signedAt: Long): String {
        val message = "${IdentityStore.DOMAIN}:$hid:$signedAt".toByteArray(Charsets.UTF_8)
        val signer = Ed25519Signer()
        signer.init(true, signing)
        signer.update(message, 0, message.size)
        return hex(signer.generateSignature())
    }

    /** What a hello carries to prove it comes from this key. */
    fun proof(signedAt: Long): List<Pair<String, Any>> = listOf(
        "hid" to hid,
        "from" to hid,
        "pubkey" to hex(pubkey),
        "signedAt" to signedAt,
        "signature" to sign(signedAt),
    )
}

object IdentityStore {
    const val DOMAIN = "hum-bee-socket-v1"
    private val BEE_ID = Regex("^bee_[0-9a-f]{64}$")

    fun load(dir: File): BeeKey {
        val id = File(dir, "bee").readText().trim()
        require(BEE_ID.matches(id)) { "${File(dir, "bee")} names no bee: $id" }
        val seed = File(dir, "bees/$id.key").readBytes()
        require(seed.size == 32) { "the key filed as $id is ${seed.size} bytes, not 32" }
        val key = BeeKey(seed)
        require(key.hid == id) { "the key filed as $id is not that bee's key" }
        return key
    }
}

internal fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

internal fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }
