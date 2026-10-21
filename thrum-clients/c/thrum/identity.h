/* thrum identity — a bee's own identity, in C.
 *
 * The mirror of hum-identity: an Ed25519 keypair, persisted per kind,
 * whose public key hashes to a role-prefixed hid. The bee proves who it
 * is by signing its hello, so the daemon can tell one participant from
 * another — and revoke one.
 */

#ifndef THRUM_IDENTITY_H
#define THRUM_IDENTITY_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define THRUM_HID_LEN 70
#define THRUM_PUBKEY_HEX_LEN 65
#define THRUM_SIG_HEX_LEN 129

typedef struct {
    uint8_t seed[32];
    uint8_t pubkey[32];
    uint8_t seckey[64];
    char hid[THRUM_HID_LEN];
} thrum_identity;

/* Load the key for a kind, or mint one if there is none. prefix is the
   role tag: "wbee" or "fbee". Returns false on failure. */
bool thrum_identity_load_or_mint(const char *kind, const char *prefix, thrum_identity *out);

/* Sign the socket hello for this identity. pubkey_hex and sig_hex must
   hold THRUM_PUBKEY_HEX_LEN and THRUM_SIG_HEX_LEN bytes. */
bool thrum_identity_sign_hello(const thrum_identity *identity, int64_t signed_at_ms,
                               char *pubkey_hex, char *sig_hex);

/* The message a socket hello signs, in the same bytes the Rust signs. */
int thrum_identity_socket_hello_message(const char *hid, int64_t signed_at_ms, char *out,
                                        size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* THRUM_IDENTITY_H */
