package hum.hive.wifip2p

import java.io.File
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.spec.PKCS8EncodedKeySpec
import java.security.spec.X509EncodedKeySpec
import java.security.KeyFactory
import java.security.KeyPair
import java.security.Security
import javax.crypto.Cipher

/**
 * Persisted Ed25519 identity for this bee.
 *
 * Mirrors hives/common/src/identity.rs and hives/openai-server/src/identity.ts
 * so the hid is byte-identical across all hive languages.
 *
 * The 32-byte seed is stored at:
 *   $XDG_STATE_HOME/hum/bees/wifi-p2p.key
 *   (fallback: ~/.local/state/hum/bees/wifi-p2p.key)
 *
 * On Android, this resolves to the app's internal data directory.
 */
object IdentityStore {
    private val PKCS8_ED25519_PREFIX = byteArrayOf(
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
        0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    )

    private var cachedHid: String? = null
    private var cachedKeyPair: KeyPair? = null

    /**
     * Load or mint the Ed25519 key pair and return the canonical hid.
     *
     * hid format: `fbee_<sha256(pubkey)>` — same prefix as Rust/TS hives.
     */
    fun getHid(dataDir: String): String {
        cachedHid?.let { return it }

        val keyFile = File(dataDir, "hum/bees/wifi-p2p.key")
        val seed: ByteArray = if (keyFile.exists() && keyFile.length() == 32L) {
            keyFile.readBytes()
        } else {
            val newSeed = generateEd25519Seed()
            keyFile.parentFile.mkdirs()
            keyFile.writeBytes(newSeed)
            newSeed
        }

        // Rebuild key pair from seed
        val keyFactory = KeyFactory.getInstance("Ed25519")
        val privKey = keyFactory.generatePrivate(PKCS8EncodedKeySpec(PKCS8_ED25519_PREFIX + seed))
        val pubKey = keyFactory.generatePublic(X509EncodedKeySpec(seed.copyOfRange(0, 32)))
        cachedKeyPair = KeyPair(pubKey, privKey)

        val pubRaw = pubKey.encoded
        val digest = MessageDigest.getInstance("SHA-256")
        digest.update(pubRaw)
        val hex = digest.digest().joinToString("") { "%02x".format(it) }
        cachedHid = "fbee_$hex"
        return cachedHid!!
    }

    fun getKeyPair(dataDir: String): KeyPair {
        cachedKeyPair?.let { return it }
        getHid(dataDir) // ensures key is loaded
        return cachedKeyPair!!
    }

    private fun generateEd25519Seed(): ByteArray {
        // Use Android's built-in Ed25519 keygen
        val kpg = KeyPairGenerator.getInstance("Ed25519")
        val kp = kpg.generateKeyPair()
        // Extract 32-byte seed from PKCS#8 private key
        val encoded = kp.private.encoded
        return encoded.copyOfRange(encoded.size - 32, encoded.size)
    }
}
