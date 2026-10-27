package hum.hive.wifip2p

import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class IdentityTest {
    @get:Rule val tmp = TemporaryFolder()

    private val seed = ByteArray(32) { it.toByte() }

    // hum-identity's BeeKey::from_seed(0..32).sign_socket_hello(1_700_000_000_000)
    private val hid = "bee_56475aa75463474c0285df5dbf2bcab73da651358839e9b77481b2eab107708c"
    private val pubkey = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8"
    private val signature =
        "adadb3bb26ffdb6c30d22fe0e5bf9da35322d1e98667c547248cba8318b911279ed803f28a9f56dc283d7e56b921636af5e25ac36e8243b0ba54fb2fd506280b"

    @Test
    fun theKeySignsAsHumIdentitySigns() {
        val key = BeeKey(seed)
        assertEquals(hid, key.hid)
        assertEquals(pubkey, hex(key.pubkey))
        assertEquals(signature, key.sign(1_700_000_000_000))
    }

    @Test
    fun theSignatureIsOverTheDomainTheHidAndTheTime() {
        val key = BeeKey(seed)
        val verify = { message: String, sig: String ->
            val bytes = message.toByteArray()
            Ed25519Signer().run {
                init(false, Ed25519PublicKeyParameters(key.pubkey, 0))
                update(bytes, 0, bytes.size)
                verifySignature(unhex(sig))
            }
        }
        val sig = key.sign(1_700_000_000_000)
        assertTrue(verify("hum-bee-socket-v1:$hid:1700000000000", sig))
        assertFalse(verify("hum-bee-socket-v1:$hid:1700000000001", sig))
        assertFalse(verify("hum-bee-socket-v1:bee_${"0".repeat(64)}:1700000000000", sig))
    }

    @Test
    fun theProofIsWhatHumIdentityPutsOnTheHello() {
        val proof = BeeKey(seed).proof(1_700_000_000_000).toMap()
        assertEquals(mapOf("hid" to hid, "from" to hid, "pubkey" to pubkey, "signedAt" to 1_700_000_000_000L, "signature" to signature), proof)
    }

    @Test
    fun aBeeLoadsFromWhereHumFiledIt() {
        val dir = file(hid, seed)
        assertEquals(hid, IdentityStore.load(dir).hid)
    }

    @Test
    fun aKeyFiledUnderAnotherIdIsRefused() {
        val other = "bee_" + "a".repeat(64)
        val dir = file(other, seed)
        assertThrows(IllegalArgumentException::class.java) { IdentityStore.load(dir) }
    }

    @Test
    fun aKeyThatIsNoSeedOrAnIdThatIsNoBeeIsRefused() {
        assertThrows(IllegalArgumentException::class.java) { IdentityStore.load(file(hid, ByteArray(64))) }
        assertThrows(IllegalArgumentException::class.java) { IdentityStore.load(file("humd_" + "a".repeat(64), seed)) }
        assertThrows(IllegalArgumentException::class.java) { IdentityStore.load(file("../bee_x", seed)) }
    }

    private fun file(id: String, key: ByteArray): File {
        val dir = tmp.newFolder()
        File(dir, "bee").writeText("$id\n")
        File(dir, "bees").mkdirs()
        if (!id.contains('/')) File(dir, "bees/$id.key").writeBytes(key)
        return dir
    }

    private fun unhex(s: String) = ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
}
