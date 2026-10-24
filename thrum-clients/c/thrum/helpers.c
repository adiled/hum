/* thrum helpers: sigil, rid, dusk, wane and the socket path, over
 * libsodium and cJSON. See helpers.h.
 */

#include "internal.h"

#include "helpers.h"

#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <sodium.h>

#define RUNTIME_INFO_BASENAME "runtime.json"
#define THRUM_SOCK_BASENAME "thrum.sock"
#define RUNTIME_INFO_MAX (1u << 20)
#define SIGIL_DOMAIN "sigil:"

static const char CROCKFORD[] = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

static _Thread_local char last_error[512];

const char *thrum_error(void) {
    return last_error;
}

int thrum__fail(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(last_error, sizeof last_error, fmt, ap);
    va_end(ap);
    return -1;
}

char *thrum__slurp(const char *path, size_t max, size_t *len) {
    FILE *f = fopen(path, "rb");
    char chunk[4096];
    char *buf = NULL;
    size_t n = 0;
    size_t got;

    if (!f) return NULL;
    while ((got = fread(chunk, 1, sizeof chunk, f)) > 0) {
        char *grown;
        if (n + got > max) {
            free(buf);
            fclose(f);
            return NULL;
        }
        grown = (char *)realloc(buf, n + got + 1);
        if (!grown) {
            free(buf);
            fclose(f);
            return NULL;
        }
        buf = grown;
        memcpy(buf + n, chunk, got);
        n += got;
    }
    fclose(f);
    if (!buf) {
        buf = (char *)malloc(1);
        if (!buf) return NULL;
    }
    buf[n] = '\0';
    if (len) *len = n;
    return buf;
}

static int put(cJSON *object, const char *key, cJSON *value) {
    if (!value) return thrum__fail("out of memory");
    if (cJSON_GetObjectItemCaseSensitive(object, key)) {
        if (!cJSON_ReplaceItemInObjectCaseSensitive(object, key, value)) {
            cJSON_Delete(value);
            return thrum__fail("could not set %s", key);
        }
        return 0;
    }
    if (!cJSON_AddItemToObject(object, key, value)) {
        cJSON_Delete(value);
        return thrum__fail("could not set %s", key);
    }
    return 0;
}

int thrum__set_string(cJSON *object, const char *key, const char *value) {
    return put(object, key, cJSON_CreateString(value));
}

int thrum__set_number(cJSON *object, const char *key, double value) {
    return put(object, key, cJSON_CreateNumber(value));
}

int thrum_init(void) {
    if (sodium_init() < 0) return thrum__fail("libsodium could not initialise");
    return 0;
}

int64_t thrum_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000 + (int64_t)(ts.tv_nsec / 1000000);
}

int64_t thrum_dusk_in(int64_t ms) {
    return thrum_now_ms() + ms;
}

bool thrum_is_dusk(const cJSON *tone) {
    const cJSON *dusk = cJSON_GetObjectItemCaseSensitive(tone, "dusk");
    if (!cJSON_IsNumber(dusk)) return false;
    return (double)thrum_now_ms() > dusk->valuedouble;
}

void thrum_sigil(const char *sid, char out[THRUM_SIGIL_LEN]) {
    crypto_hash_sha256_state state;
    unsigned char hash[crypto_hash_sha256_BYTES];

    crypto_hash_sha256_init(&state);
    crypto_hash_sha256_update(&state, (const unsigned char *)SIGIL_DOMAIN, strlen(SIGIL_DOMAIN));
    crypto_hash_sha256_update(&state, (const unsigned char *)sid, strlen(sid));
    crypto_hash_sha256_final(&state, hash);
    sodium_bin2hex(out, THRUM_SIGIL_LEN, hash, 6);
}

void thrum_humid_encode(const uint8_t bytes[32], char out[THRUM_RID_LEN]) {
    size_t i;
    for (i = 0; i < 52; i++) {
        unsigned value = 0;
        size_t k;
        for (k = 0; k < 5; k++) {
            size_t bit = i * 5 + k;
            unsigned b = bit < 256 ? (unsigned)(bytes[bit / 8] >> (7 - bit % 8)) & 1u : 0u;
            value = (value << 1) | b;
        }
        out[i] = CROCKFORD[value];
    }
    out[52] = '\0';
}

void thrum_rid(char out[THRUM_RID_LEN]) {
    uint8_t bytes[32];
    uint64_t ms = (uint64_t)thrum_now_ms();
    int i;

    for (i = 0; i < 6; i++) bytes[5 - i] = (uint8_t)(ms >> (8 * i));
    (void)thrum_init();
    randombytes_buf(bytes + 6, 26);
    thrum_humid_encode(bytes, out);
}

bool thrum_is_valid_rid(const char *value) {
    size_t i;
    if (!value || strlen(value) != 52) return false;
    for (i = 0; i < 52; i++) {
        if (!strchr(CROCKFORD, value[i])) return false;
    }
    return true;
}

int thrum_state_dir(char *out, size_t cap) {
    const char *xdg = getenv("XDG_STATE_HOME");
    const char *home = getenv("HOME");
    int n;

    if (xdg && xdg[0]) {
        n = snprintf(out, cap, "%s/hum", xdg);
    } else if (home && home[0]) {
        n = snprintf(out, cap, "%s/.local/state/hum", home);
    } else {
        return thrum__fail("neither XDG_STATE_HOME nor HOME is set");
    }
    if (n < 0 || (size_t)n >= cap) return thrum__fail("the state directory path is too long");
    return 0;
}

static int copy_path(char *out, size_t cap, const char *path) {
    int n = snprintf(out, cap, "%s", path);
    if (n < 0 || (size_t)n >= cap) return thrum__fail("socket path is too long: %s", path);
    return 0;
}

static int rendezvous(const char *state, char *out, size_t cap) {
    char path[1100];
    size_t len = 0;
    char *text;
    cJSON *doc;
    const cJSON *socket;
    int rc = 1;

    if (snprintf(path, sizeof path, "%s/%s", state, RUNTIME_INFO_BASENAME) >= (int)sizeof path) {
        return 1;
    }
    text = thrum__slurp(path, RUNTIME_INFO_MAX, &len);
    if (!text) return 1;
    doc = cJSON_ParseWithLength(text, len);
    socket = cJSON_GetObjectItemCaseSensitive(doc, "socket");
    if (cJSON_IsString(socket) && socket->valuestring[0]) rc = copy_path(out, cap, socket->valuestring);
    cJSON_Delete(doc);
    free(text);
    return rc;
}

int thrum_default_socket_path(char *out, size_t cap) {
    const char *env;
    char state[1024];
    int rc;
    int n;

    if ((env = getenv("HUM_THRUM_SOCK")) && env[0]) return copy_path(out, cap, env);
    if ((env = getenv("HUM_SOCKET")) && env[0]) return copy_path(out, cap, env);
    if (thrum_state_dir(state, sizeof state) != 0) return -1;
    rc = rendezvous(state, out, cap);
    if (rc != 1) return rc;
    n = snprintf(out, cap, "%s/%s", state, THRUM_SOCK_BASENAME);
    if (n < 0 || (size_t)n >= cap) return thrum__fail("socket path is too long");
    return 0;
}

struct wane_slot {
    char *sigil;
    int64_t value;
};

struct thrum_wane {
    pthread_mutex_t lock;
    struct wane_slot *slots;
    size_t count;
    size_t cap;
};

thrum_wane *thrum_wane_new(void) {
    thrum_wane *wane = (thrum_wane *)calloc(1, sizeof *wane);
    if (!wane) {
        thrum__fail("out of memory");
        return NULL;
    }
    pthread_mutex_init(&wane->lock, NULL);
    return wane;
}

void thrum_wane_free(thrum_wane *wane) {
    size_t i;
    if (!wane) return;
    for (i = 0; i < wane->count; i++) free(wane->slots[i].sigil);
    free(wane->slots);
    pthread_mutex_destroy(&wane->lock);
    free(wane);
}

static int64_t *slot_of(thrum_wane *wane, const char *sigil, bool create) {
    size_t i;
    char *copy;
    for (i = 0; i < wane->count; i++) {
        if (strcmp(wane->slots[i].sigil, sigil) == 0) return &wane->slots[i].value;
    }
    if (!create) return NULL;
    if (wane->count == wane->cap) {
        size_t cap = wane->cap ? wane->cap * 2 : 8;
        struct wane_slot *grown =
            (struct wane_slot *)realloc(wane->slots, cap * sizeof *grown);
        if (!grown) return NULL;
        wane->slots = grown;
        wane->cap = cap;
    }
    copy = strdup(sigil);
    if (!copy) return NULL;
    wane->slots[wane->count].sigil = copy;
    wane->slots[wane->count].value = 0;
    return &wane->slots[wane->count++].value;
}

int64_t thrum_wane_get(thrum_wane *wane, const char *sigil) {
    int64_t *slot;
    int64_t value;
    pthread_mutex_lock(&wane->lock);
    slot = slot_of(wane, sigil, false);
    value = slot ? *slot : 0;
    pthread_mutex_unlock(&wane->lock);
    return value;
}

int64_t thrum_wane_tick(thrum_wane *wane, const char *sigil) {
    int64_t *slot;
    int64_t value;
    pthread_mutex_lock(&wane->lock);
    slot = slot_of(wane, sigil, true);
    value = slot ? ++*slot : -1;
    pthread_mutex_unlock(&wane->lock);
    if (value < 0) thrum__fail("out of memory");
    return value;
}

int thrum_wane_set(thrum_wane *wane, const char *sigil, int64_t value) {
    int64_t *slot;
    pthread_mutex_lock(&wane->lock);
    slot = slot_of(wane, sigil, true);
    if (slot) *slot = value;
    pthread_mutex_unlock(&wane->lock);
    return slot ? 0 : thrum__fail("out of memory");
}

bool thrum_wane_behind(thrum_wane *wane, const char *sigil, int64_t remote) {
    return remote > thrum_wane_get(wane, sigil);
}
