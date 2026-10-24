/* thrum identity: a C bee's key, entered by its hive and signed into its
 * hello.
 *
 * A bee is a key its hive entered. Entering goes through the hum CLI, so
 * there is one implementation of it: thrum_bee_enter runs
 * `hum bee enter --hive <hive> --as <label>` and reads back the id, which
 * is the same bee every time for one hive and label. thrum_bee_load reads
 * the 32-byte seed entering filed at $XDG_STATE_HOME/hum/bees/<id>.key and
 * refuses a key that is not that id's. thrum_bee_sign_hello puts hid,
 * from, pubkey, signedAt and signature on a hello, the signature being
 * Ed25519 over
 *
 *     hum-bee-socket-v1:<hid>:<signedAt>
 *
 * the bytes hum-identity signs. An id is bee_ and the hex sha256 of the
 * public key. The keys are libsodium's.
 */

#ifndef THRUM_IDENTITY_H
#define THRUM_IDENTITY_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <cjson/cJSON.h>

#ifdef __cplusplus
extern "C" {
#endif

#define THRUM_BEE_ID_LEN 69
#define THRUM_BEE_SEED_LEN 32
#define THRUM_BEE_PUBKEY_LEN 32
#define THRUM_BEE_SECRET_LEN 64
#define THRUM_BEE_SOCKET_DOMAIN "hum-bee-socket-v1"

typedef struct {
    char id[THRUM_BEE_ID_LEN];
    unsigned char pubkey[THRUM_BEE_PUBKEY_LEN];
    unsigned char secret[THRUM_BEE_SECRET_LEN];
} thrum_bee;

bool thrum_is_plain_name(const char *name);
bool thrum_bee_id_is_valid(const char *id);

int thrum_bee_enter(const char *hive, const char *label, char id[THRUM_BEE_ID_LEN]);
int thrum_bee_load(const char *id, thrum_bee *bee);
int thrum_bee_enter_as(const char *hive, const char *label, thrum_bee *bee);

int thrum_bee_from_seed(const unsigned char seed[THRUM_BEE_SEED_LEN], thrum_bee *bee);
int thrum_bee_key_path(const char *id, char *out, size_t cap);
int thrum_bee_hello_message(const char *id, int64_t signed_at_ms, char *out, size_t cap);

int thrum_bee_sign_hello(const thrum_bee *bee, cJSON *hello);
int thrum_bee_sign_hello_at(const thrum_bee *bee, cJSON *hello, int64_t signed_at_ms);

void thrum_bee_forget(thrum_bee *bee);

#ifdef __cplusplus
}
#endif

#endif
