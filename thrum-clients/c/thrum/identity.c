/* thrum identity: entering through the hum CLI, loading the filed key,
 * and signing the hello, over libsodium. See identity.h.
 */

#include "internal.h"

#include "identity.h"

#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include <sodium.h>

#include "helpers.h"

_Static_assert(THRUM_BEE_SEED_LEN == crypto_sign_SEEDBYTES, "an entered key is an Ed25519 seed");
_Static_assert(THRUM_BEE_PUBKEY_LEN == crypto_sign_PUBLICKEYBYTES, "Ed25519 public key");
_Static_assert(THRUM_BEE_SECRET_LEN == crypto_sign_SECRETKEYBYTES, "Ed25519 secret key");

#define BEE_PREFIX "bee_"
#define BEE_PREFIX_LEN 4
#define PLAIN_NAME_MAX 128
#define ENTER_OUTPUT_MAX 512

extern char **environ;

bool thrum_is_plain_name(const char *name) {
    size_t i, len;
    if (!name) return false;
    len = strlen(name);
    if (len == 0 || len > PLAIN_NAME_MAX || name[0] == '.') return false;
    for (i = 0; i < len; i++) {
        char c = name[i];
        bool alnum = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9');
        if (!alnum && c != '-' && c != '_' && c != '.') return false;
    }
    return true;
}

bool thrum_bee_id_is_valid(const char *id) {
    size_t i;
    if (!id || strlen(id) != THRUM_BEE_ID_LEN - 1) return false;
    if (strncmp(id, BEE_PREFIX, BEE_PREFIX_LEN) != 0) return false;
    for (i = BEE_PREFIX_LEN; i < THRUM_BEE_ID_LEN - 1; i++) {
        char c = id[i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return false;
    }
    return true;
}

int thrum_bee_key_path(const char *id, char *out, size_t cap) {
    char state[1024];
    int n;
    if (!thrum_bee_id_is_valid(id)) return thrum__fail("%s is not a bee id", id ? id : "(null)");
    if (thrum_state_dir(state, sizeof state) != 0) return -1;
    n = snprintf(out, cap, "%s/bees/%s.key", state, id);
    if (n < 0 || (size_t)n >= cap) return thrum__fail("bee key path is too long");
    return 0;
}

static bool is_executable(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode) && access(path, X_OK) == 0;
}

static int find_hum(char *out, size_t cap) {
    const char *path = getenv("PATH");
    const char *home = getenv("HOME");
    int n;

    if (path) {
        const char *dir = path;
        for (;;) {
            const char *colon = strchr(dir, ':');
            size_t len = colon ? (size_t)(colon - dir) : strlen(dir);
            if (len > 0) {
                n = snprintf(out, cap, "%.*s/hum", (int)len, dir);
                if (n > 0 && (size_t)n < cap && is_executable(out)) return 0;
            }
            if (!colon) break;
            dir = colon + 1;
        }
    }
    if (home && home[0]) {
        n = snprintf(out, cap, "%s/.local/bin/hum", home);
        if (n > 0 && (size_t)n < cap && is_executable(out)) return 0;
    }
    return thrum__fail("hum is not on PATH or in ~/.local/bin; a bee cannot enter without it");
}

static void trim(char *text) {
    size_t len = strlen(text);
    size_t start = 0;
    while (len > 0 && (text[len - 1] == '\n' || text[len - 1] == '\r' || text[len - 1] == ' ' ||
                       text[len - 1] == '\t')) {
        text[--len] = '\0';
    }
    while (text[start] == ' ' || text[start] == '\t' || text[start] == '\n') start++;
    if (start > 0) memmove(text, text + start, len - start + 1);
}

static int run_enter(const char *hum, const char *hive, const char *label, char *out, size_t cap) {
    char *argv[8];
    int pipefd[2];
    posix_spawn_file_actions_t actions;
    pid_t pid;
    int rc;
    int status = 0;
    size_t n = 0;

    argv[0] = (char *)"hum";
    argv[1] = (char *)"bee";
    argv[2] = (char *)"enter";
    argv[3] = (char *)"--hive";
    argv[4] = (char *)hive;
    argv[5] = (char *)"--as";
    argv[6] = (char *)label;
    argv[7] = NULL;

    if (pipe(pipefd) != 0) return thrum__fail("pipe: %s", strerror(errno));
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, pipefd[1], STDOUT_FILENO);
    posix_spawn_file_actions_addclose(&actions, pipefd[0]);
    posix_spawn_file_actions_addclose(&actions, pipefd[1]);
    rc = posix_spawn(&pid, hum, &actions, NULL, argv, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(pipefd[1]);
    if (rc != 0) {
        close(pipefd[0]);
        return thrum__fail("could not run %s: %s", hum, strerror(rc));
    }

    for (;;) {
        char sink[256];
        ssize_t got;
        if (n < cap - 1) {
            got = read(pipefd[0], out + n, cap - 1 - n);
        } else {
            got = read(pipefd[0], sink, sizeof sink);
        }
        if (got < 0 && errno == EINTR) continue;
        if (got <= 0) break;
        if (n < cap - 1) n += (size_t)got;
    }
    out[n] = '\0';
    close(pipefd[0]);

    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) return thrum__fail("waitpid: %s", strerror(errno));
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return thrum__fail("hum bee enter --hive %s --as %s failed (status %d)", hive, label,
                           WIFEXITED(status) ? WEXITSTATUS(status) : -1);
    }
    return 0;
}

int thrum_bee_enter(const char *hive, const char *label, char id[THRUM_BEE_ID_LEN]) {
    char hum[1100];
    char out[ENTER_OUTPUT_MAX];

    if (!thrum_is_plain_name(hive)) return thrum__fail("%s is not a plain hive name", hive ? hive : "(null)");
    if (!thrum_is_plain_name(label)) return thrum__fail("%s is not a plain label", label ? label : "(null)");
    if (find_hum(hum, sizeof hum) != 0) return -1;
    if (run_enter(hum, hive, label, out, sizeof out) != 0) return -1;
    trim(out);
    if (!thrum_bee_id_is_valid(out)) return thrum__fail("hum bee enter answered \"%s\", not a bee id", out);
    memcpy(id, out, THRUM_BEE_ID_LEN);
    return 0;
}

int thrum_bee_from_seed(const unsigned char seed[THRUM_BEE_SEED_LEN], thrum_bee *bee) {
    unsigned char hash[crypto_hash_sha256_BYTES];

    if (thrum_init() != 0) return -1;
    if (crypto_sign_seed_keypair(bee->pubkey, bee->secret, seed) != 0) {
        return thrum__fail("could not derive the bee's keypair");
    }
    crypto_hash_sha256(hash, bee->pubkey, sizeof bee->pubkey);
    memcpy(bee->id, BEE_PREFIX, BEE_PREFIX_LEN);
    sodium_bin2hex(bee->id + BEE_PREFIX_LEN, THRUM_BEE_ID_LEN - BEE_PREFIX_LEN, hash, sizeof hash);
    return 0;
}

int thrum_bee_load(const char *id, thrum_bee *bee) {
    char path[1200];
    unsigned char seed[THRUM_BEE_SEED_LEN + 1];
    size_t got;
    FILE *f;
    int rc;

    if (thrum_bee_key_path(id, path, sizeof path) != 0) return -1;
    f = fopen(path, "rb");
    if (!f) return thrum__fail("read bee key %s: %s", path, strerror(errno));
    got = fread(seed, 1, sizeof seed, f);
    fclose(f);
    if (got != THRUM_BEE_SEED_LEN) {
        sodium_memzero(seed, sizeof seed);
        return thrum__fail("bee key at %s is %s32 bytes, expected 32", path,
                           got > THRUM_BEE_SEED_LEN ? "more than " : "less than ");
    }
    rc = thrum_bee_from_seed(seed, bee);
    sodium_memzero(seed, sizeof seed);
    if (rc != 0) return -1;
    if (strcmp(bee->id, id) != 0) {
        thrum_bee_forget(bee);
        return thrum__fail("bee key at %s is not the key of %s", path, id);
    }
    return 0;
}

int thrum_bee_enter_as(const char *hive, const char *label, thrum_bee *bee) {
    char id[THRUM_BEE_ID_LEN];
    if (thrum_bee_enter(hive, label, id) != 0) return -1;
    return thrum_bee_load(id, bee);
}

int thrum_bee_hello_message(const char *id, int64_t signed_at_ms, char *out, size_t cap) {
    int n = snprintf(out, cap, "%s:%s:%lld", THRUM_BEE_SOCKET_DOMAIN, id, (long long)signed_at_ms);
    if (n < 0 || (size_t)n >= cap) return thrum__fail("hello message does not fit");
    return n;
}

int thrum_bee_sign_hello_at(const thrum_bee *bee, cJSON *hello, int64_t signed_at_ms) {
    char message[160];
    unsigned char signature[crypto_sign_BYTES];
    char pubkey_hex[THRUM_BEE_PUBKEY_LEN * 2 + 1];
    char signature_hex[crypto_sign_BYTES * 2 + 1];
    int n;

    if (!cJSON_IsObject(hello)) return thrum__fail("a hello is a JSON object");
    if (thrum_init() != 0) return -1;
    n = thrum_bee_hello_message(bee->id, signed_at_ms, message, sizeof message);
    if (n < 0) return -1;
    if (crypto_sign_detached(signature, NULL, (const unsigned char *)message,
                             (unsigned long long)n, bee->secret) != 0) {
        return thrum__fail("could not sign the hello");
    }
    sodium_bin2hex(pubkey_hex, sizeof pubkey_hex, bee->pubkey, sizeof bee->pubkey);
    sodium_bin2hex(signature_hex, sizeof signature_hex, signature, sizeof signature);

    if (thrum__set_string(hello, "hid", bee->id) != 0) return -1;
    if (thrum__set_string(hello, "from", bee->id) != 0) return -1;
    if (thrum__set_string(hello, "pubkey", pubkey_hex) != 0) return -1;
    if (thrum__set_number(hello, "signedAt", (double)signed_at_ms) != 0) return -1;
    if (thrum__set_string(hello, "signature", signature_hex) != 0) return -1;
    return 0;
}

int thrum_bee_sign_hello(const thrum_bee *bee, cJSON *hello) {
    return thrum_bee_sign_hello_at(bee, hello, thrum_now_ms());
}

void thrum_bee_forget(thrum_bee *bee) {
    if (bee) sodium_memzero(bee, sizeof *bee);
}
