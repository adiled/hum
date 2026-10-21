/* thrum identity — a bee's own identity, in C. See identity.h. */

#include "identity.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "helpers.h"
#include "ed25519/ed25519.h"

#define BEE_SOCKET_DOMAIN "hum-bee-socket-v1"

static const char HEXD[] = "0123456789abcdef";

static void to_hex(const uint8_t *bytes, size_t len, char *out) {
    size_t i;
    for (i = 0; i < len; i++) {
        out[i * 2] = HEXD[bytes[i] >> 4];
        out[i * 2 + 1] = HEXD[bytes[i] & 0x0f];
    }
    out[len * 2] = '\0';
}

static bool key_path(const char *kind, char *out, size_t cap) {
    const char *state = getenv("XDG_STATE_HOME");
    const char *home = getenv("HOME");
    if (!kind || !kind[0]) return false;
    if (state && state[0]) {
        snprintf(out, cap, "%s/hum/bees/%s.key", state, kind);
    } else if (home && home[0]) {
        snprintf(out, cap, "%s/.local/state/hum/bees/%s.key", home, kind);
    } else {
        return false;
    }
    return true;
}

static bool make_path(const char *path) {
    char buf[1024];
    char *slash;
    snprintf(buf, sizeof buf, "%s", path);
    slash = strrchr(buf, '/');
    if (!slash) return true;
    *slash = '\0';
    if (buf[0] == '\0') return true;
    if (!make_path(buf)) return false;
    if (mkdir(path, 0700) == 0) return true;
    return errno == EEXIST;
}

static bool ensure_dir(const char *path) {
    char buf[1024];
    char *slash;
    snprintf(buf, sizeof buf, "%s", path);
    slash = strrchr(buf, '/');
    if (!slash) return false;
    *slash = '\0';
    if (buf[0] == '\0') return true;
    return make_path(buf);
}

static bool write_seed(const char *path, const uint8_t seed[32]) {
    char tmp[1100];
    FILE *f;
    snprintf(tmp, sizeof tmp, "%s.tmp", path);
    f = fopen(tmp, "wb");
    if (!f) return false;
    if (fwrite(seed, 1, 32, f) != 32) {
        fclose(f);
        remove(tmp);
        return false;
    }
    fclose(f);
    chmod(tmp, 0600);
    return rename(tmp, path) == 0;
}

bool thrum_identity_load_or_mint(const char *kind, const char *prefix, thrum_identity *out) {
    char path[1100];
    uint8_t seed[32];
    FILE *f;
    bool minted = false;
    thrum_sha256 digest;
    uint8_t hash[32];
    char hex[65];

    if (!key_path(kind, path, sizeof path)) return false;

    f = fopen(path, "rb");
    if (f) {
        size_t got = fread(seed, 1, 32, f);
        fclose(f);
        if (got != 32) return false;
    } else {
        if (!ensure_dir(path)) return false;
        if (ed25519_create_seed(seed) != 0) return false;
        if (!write_seed(path, seed)) return false;
        minted = true;
    }

    ed25519_create_keypair(out->pubkey, out->seckey, seed);
    memcpy(out->seed, seed, 32);

    thrum_sha256_init(&digest);
    thrum_sha256_update(&digest, out->pubkey, 32);
    thrum_sha256_final(&digest, hash);
    to_hex(hash, 32, hex);
    snprintf(out->hid, sizeof out->hid, "%s_%s", prefix, hex);

    if (minted) {
        fprintf(stderr, "thrum: minted identity %s for %s\n", out->hid, kind);
    }
    return true;
}

int thrum_identity_socket_hello_message(const char *hid, int64_t signed_at_ms, char *out,
                                        size_t cap) {
    int written = snprintf(out, cap, "%s:%s:%lld", BEE_SOCKET_DOMAIN, hid,
                            (long long)signed_at_ms);
    if (written < 0 || (size_t)written >= cap) return -1;
    return written;
}

bool thrum_identity_sign_hello(const thrum_identity *identity, int64_t signed_at_ms,
                               char *pubkey_hex, char *sig_hex) {
    char message[256];
    uint8_t signature[64];
    int len = thrum_identity_socket_hello_message(identity->hid, signed_at_ms, message,
                                                  sizeof message);
    if (len < 0) return false;
    ed25519_sign(signature, (const uint8_t *)message, (size_t)len, identity->pubkey,
                  identity->seckey);
    to_hex(identity->pubkey, 32, pubkey_hex);
    to_hex(signature, 64, sig_hex);
    return true;
}
