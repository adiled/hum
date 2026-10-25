/* dwarfstar-bee: the DwarfStar engine as a hum worker.
 *
 * The bee reaches humd through the thrum C client, which runs
 * `hum bee link` for it, and answers prompts from the engine linked into
 * this process: a prompt tone becomes a chat prompt, the chat prompt becomes
 * tokens, and the tokens unfurl as a text petal until the bloom wilts. No
 * HTTP and no subprocess in between.
 *
 * A unit of the hive's Orchfile pins its bee to one model, so every model
 * is its own bee with its own key, ids and headroom; without a pin the bee
 * serves every model under DWARFSTAR_DIR. All of a hive's bees share one
 * GPU (gpu.h): a bee takes it before loading its model, gives it back
 * after DWARFSTAR_IDLE_MS idle or when another model's bee wants it, and
 * claims one slot of headroom only while the GPU is its own or free, so a
 * peer routes to the model that can start now. A local prompt for a model
 * whose bee lacks the GPU still runs: it waits for the handoff.
 *
 * The bee outlives humd: when humd restarts, the link reconnects and the
 * bee keeps its model loaded and drops the blooms the gone humd gave it.
 * Only a refusal, or the link ending, ends the bee, non-zero, for its
 * unit to restart.
 *
 * The engine grows one bloom at a time, so prompts queue and one thread
 * takes them in order and tends the GPU between them, while the client's
 * thread reads the link. A cancel stops its sid's bloom, waiting or in
 * flight, and drops its sid's queued blooms. The bee is stateless: every
 * prompt carries what it needs, as the text petals of its content, else
 * its text, and the engine's session reuses whatever of it the KV cache
 * already holds. An unfurl always carries whole UTF-8 characters, even
 * when a token ends inside one. A bloom wilts with its nectar: the tokens
 * in and out.
 *
 * Environment:
 *   DWARFSTAR_MODEL        pin to this GGUF, absolute or under DWARFSTAR_DIR
 *   DWARFSTAR_MODELS       the ids a pinned bee answers to (default: by name)
 *   DWARFSTAR_LABEL        the label the bee enters as (default main)
 *   DWARFSTAR_DIR          where the GGUFs live (default ~/ds4)
 *   DWARFSTAR_CTX          context size in tokens (default 32768)
 *   DWARFSTAR_MAX_TOKENS   cap on one bloom (default 4096)
 *   DWARFSTAR_TEMPERATURE  sampling temperature (default ds4's)
 *   DWARFSTAR_THREADS      engine CPU threads (default ds4's)
 *   DWARFSTAR_SSD_STREAMING  1 to stream experts from SSD
 *   DWARFSTAR_IDLE_MS      give the GPU back after this long idle (default 300000)
 *   DWARFSTAR_WAIT_MS      how long a bloom waits for the GPU (default 300000)
 *   DWARFSTAR_GPU_LOCK     the hive-wide GPU lock; the Orchfile puts it in
 *                          the hive's state, ${HUM_HIVE_STATE}/gpu.lock
 *   DWARFSTAR_METAL_DIR    the .metal kernels (default metal/ beside the bee)
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

#include <thrum/thrum.h>

#include "ds4_bridge.h"
#include "gpu.h"
#include "models.h"
#include "shaders.h"

#define HIVE "dwarfstar"
#define VERSION "0.33.1"
#define SOURCE "https://github.com/adiled/hum/tree/main/hives/dwarfstar"
#define WIRE "dwarfstar/core"
#define TICK_MS 500
#define YIELD_AFTER_MS 60000

struct bloom {
    char *sid;
    char *rid;
    char *model_path;
    char *system;
    ds4_message *messages;
    size_t count;
    struct bloom *next;
};

struct stream {
    const struct bloom *bloom;
    char *held;
    size_t len;
    size_t cap;
};

static thrum_bee *g_bee;
static ds4_bridge *g_bridge;
static dwarfstar_gpu *g_gpu;
static ds4_models g_models;
static int g_max_tokens = 4096;
static float g_temperature = -1.0f;
static int64_t g_idle_ms = 300000;
static int64_t g_wait_ms = 300000;

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_ready = PTHREAD_COND_INITIALIZER;
static struct bloom *g_head;
static struct bloom *g_tail;
static const char *g_current_sid;
static bool g_closing;

static atomic_int g_free_slots = -1;
static const char *g_label = "main";
static int64_t g_last_bloom_ms;

static const char *str_of(const cJSON *tone, const char *key) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static char *dup_of(const char *text) {
    return text ? strdup(text) : NULL;
}

static const char *env_of(const char *name) {
    const char *value = getenv(name);
    return value && value[0] ? value : NULL;
}

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static cJSON *tone_new(const char *chi, const char *rid, const char *sid) {
    cJSON *tone = cJSON_CreateObject();
    if (!tone) return NULL;
    cJSON_AddStringToObject(tone, "chi", chi);
    if (rid) cJSON_AddStringToObject(tone, "rid", rid);
    if (sid) cJSON_AddStringToObject(tone, "sid", sid);
    return tone;
}

static void free_bloom(struct bloom *t) {
    size_t i;
    if (!t) return;
    for (i = 0; i < t->count; i++) {
        free((char *)t->messages[i].role);
        free((char *)t->messages[i].text);
    }
    free(t->messages);
    free(t->sid);
    free(t->rid);
    free(t->model_path);
    free(t->system);
    free(t);
}

static void send_tone(cJSON *tone) {
    if (!tone) return;
    if (thrum_bee_send(g_bee, tone) != 0) {
        fprintf(stderr, "dwarfstar: the link is gone; a tone was not sent\n");
    }
    cJSON_Delete(tone);
}

static cJSON *headroom_of(int free_slots) {
    cJSON *headroom = cJSON_CreateObject();
    if (!headroom) return NULL;
    cJSON_AddNumberToObject(headroom, "freeSlots", free_slots);
    cJSON_AddNumberToObject(headroom, "totalSlots", 1);
    return headroom;
}

static void claim(int free_slots, const char *kind) {
    cJSON *tone;
    bool changed = free_slots != atomic_load(&g_free_slots);
    atomic_store(&g_free_slots, free_slots);
    if (!changed && strcmp(kind, THRUM_PULSE_CELL_IDLE) == 0) return;
    tone = tone_new(THRUM_CHI_PULSE, NULL, NULL);
    if (!tone) return;
    cJSON_AddStringToObject(tone, "kind", kind);
    cJSON_AddItemToObject(tone, "headroom", headroom_of(free_slots));
    send_tone(tone);
}

static void send_error(const struct bloom *t, const char *code, const char *message) {
    cJSON *tone = tone_new(THRUM_CHI_ERROR, t->rid, t->sid);
    if (!tone) return;
    cJSON_AddStringToObject(tone, "code", code);
    cJSON_AddStringToObject(tone, "message", message);
    send_tone(tone);
}

static void send_wilt(const struct bloom *t, const char *reason, int exit_code,
                      const ds4_wilt *result) {
    cJSON *tone = tone_new(THRUM_CHI_WILT, t->rid, t->sid);
    cJSON *nectar;
    if (!tone) return;
    cJSON_AddStringToObject(tone, "reason", reason);
    cJSON_AddNumberToObject(tone, "exitCode", exit_code);
    nectar = cJSON_AddObjectToObject(tone, "nectar");
    if (nectar) {
        cJSON_AddNumberToObject(nectar, "input_tokens", result ? result->tokens_in : 0);
        cJSON_AddNumberToObject(nectar, "output_tokens", result ? result->tokens_out : 0);
    }
    send_tone(tone);
}

static void send_unfurl(const struct bloom *t, const char *text, size_t len) {
    cJSON *tone = tone_new(THRUM_CHI_UNFURL, t->rid, t->sid);
    char *grew = (char *)malloc(len + 1);
    if (!tone || !grew) {
        cJSON_Delete(tone);
        free(grew);
        return;
    }
    memcpy(grew, text, len);
    grew[len] = '\0';
    cJSON_AddNumberToObject(tone, "petal", 0);
    cJSON_AddStringToObject(tone, "kind", THRUM_PETAL_TEXT);
    cJSON_AddStringToObject(tone, "text", grew);
    free(grew);
    send_tone(tone);
}

static size_t complete_utf8(const unsigned char *bytes, size_t len) {
    size_t start = len;
    size_t tail = 0;
    size_t need;
    unsigned char lead;

    while (start > 0 && tail < 4 && (bytes[start - 1] & 0xC0) == 0x80) {
        start--;
        tail++;
    }
    if (start == 0) return len;
    lead = bytes[start - 1];
    if (lead < 0x80) need = 1;
    else if ((lead & 0xE0) == 0xC0) need = 2;
    else if ((lead & 0xF0) == 0xE0) need = 3;
    else if ((lead & 0xF8) == 0xF0) need = 4;
    else need = 1;
    return tail + 1 >= need ? len : start - 1;
}

static void on_piece(void *user, const char *text, size_t len) {
    struct stream *s = (struct stream *)user;
    size_t ready;

    if (s->len + len > s->cap) {
        size_t cap = s->cap ? s->cap : 256;
        char *grown;
        while (cap < s->len + len) cap *= 2;
        grown = (char *)realloc(s->held, cap);
        if (!grown) return;
        s->held = grown;
        s->cap = cap;
    }
    memcpy(s->held + s->len, text, len);
    s->len += len;
    ready = complete_utf8((const unsigned char *)s->held, s->len);
    if (ready == 0) return;
    send_unfurl(s->bloom, s->held, ready);
    memmove(s->held, s->held + ready, s->len - ready);
    s->len -= ready;
}

static bool bloom_stopped(void *user) {
    bool closing;
    (void)user;
    pthread_mutex_lock(&g_lock);
    closing = g_closing;
    pthread_mutex_unlock(&g_lock);
    return closing || ds4_bridge_cancelled(g_bridge);
}

static int free_slots_now(void) {
    return gpu_probe(g_gpu) == GPU_OTHER ? 0 : 1;
}

static void give_back_gpu(void) {
    ds4_bridge_unload(g_bridge);
    gpu_release(g_gpu);
    claim(free_slots_now(), THRUM_PULSE_CELL_EVICTED);
}

static int take_gpu(const struct bloom *t) {
    char err[512] = "";
    int rc;

    if (ds4_bridge_ready(g_bridge, t->model_path)) return 0;
    rc = gpu_acquire(g_gpu, g_wait_ms, bloom_stopped, NULL, err, sizeof err);
    if (rc == 1) {
        send_wilt(t, "cancelled", 0, NULL);
        return -1;
    }
    if (rc != 0) {
        send_error(t, "gpu_busy", err);
        send_wilt(t, "error", 1, NULL);
        return -1;
    }
    if (ds4_bridge_load(g_bridge, t->model_path, err, sizeof err) != 0) {
        gpu_release(g_gpu);
        claim(free_slots_now(), THRUM_PULSE_CELL_DIED);
        send_error(t, "engine_unavailable", err);
        send_wilt(t, "error", 1, NULL);
        return -1;
    }
    fprintf(stderr, "dwarfstar: loaded %s\n", t->model_path);
    claim(1, THRUM_PULSE_CELL_SPAWNED);
    return 0;
}

static void run_bloom(const struct bloom *t) {
    struct stream s = {t, NULL, 0, 0};
    ds4_wilt result;
    char err[512];
    int rc;

    if (take_gpu(t) != 0) return;
    rc = ds4_bridge_generate(g_bridge, t->system, t->messages, t->count, g_temperature,
                             g_max_tokens, on_piece, &s, &result, err, sizeof err);
    free(s.held);
    if (rc != 0) {
        send_error(t, "generation_failed", err[0] ? err : "the engine failed");
        send_wilt(t, "error", 1, &result);
        return;
    }
    send_wilt(t, result.reason, 0, &result);
}

static void tend_gpu(bool after_bloom) {
    int64_t want_age;
    bool queued;

    if (!ds4_bridge_loaded(g_bridge)) {
        claim(free_slots_now(), THRUM_PULSE_CELL_IDLE);
        return;
    }
    pthread_mutex_lock(&g_lock);
    queued = g_head != NULL;
    pthread_mutex_unlock(&g_lock);
    want_age = gpu_oldest_want_ms(g_gpu);
    if (!queued && (want_age >= 0 || now_ms() - g_last_bloom_ms >= g_idle_ms)) {
        fprintf(stderr, "dwarfstar: giving the GPU back (%s)\n",
                want_age >= 0 ? "another model wants it" : "idle");
        give_back_gpu();
    } else if (after_bloom && want_age >= YIELD_AFTER_MS) {
        fprintf(stderr, "dwarfstar: giving the GPU back after a bloom (a want waited %llds)\n",
                (long long)(want_age / 1000));
        give_back_gpu();
    }
}

static struct timespec tick_from_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    ts.tv_nsec += (long)TICK_MS * 1000000L;
    ts.tv_sec += ts.tv_nsec / 1000000000L;
    ts.tv_nsec %= 1000000000L;
    return ts;
}

static void *take_blooms(void *arg) {
    (void)arg;
    for (;;) {
        struct bloom *t;
        pthread_mutex_lock(&g_lock);
        while (!g_head && !g_closing) {
            struct timespec until = tick_from_now();
            if (pthread_cond_timedwait(&g_ready, &g_lock, &until) == ETIMEDOUT) {
                pthread_mutex_unlock(&g_lock);
                tend_gpu(false);
                pthread_mutex_lock(&g_lock);
            }
        }
        if (g_closing) {
            pthread_mutex_unlock(&g_lock);
            break;
        }
        t = g_head;
        g_head = t->next;
        if (!g_head) g_tail = NULL;
        g_current_sid = t->sid;
        ds4_bridge_arm(g_bridge);
        pthread_mutex_unlock(&g_lock);

        run_bloom(t);

        pthread_mutex_lock(&g_lock);
        g_current_sid = NULL;
        pthread_mutex_unlock(&g_lock);
        g_last_bloom_ms = now_ms();
        free_bloom(t);
        tend_gpu(true);
    }
    ds4_bridge_unload(g_bridge);
    gpu_release(g_gpu);
    return NULL;
}

static int add_message(struct bloom *t, const char *role, const char *text) {
    ds4_message *grown = (ds4_message *)realloc(t->messages, (t->count + 1) * sizeof *grown);
    if (!grown) return -1;
    t->messages = grown;
    t->messages[t->count].role = dup_of(role);
    t->messages[t->count].text = dup_of(text);
    t->count++;
    return 0;
}

static int read_messages(const cJSON *tone, struct bloom *t) {
    const char *text = str_of(tone, "text");
    const cJSON *content = cJSON_GetObjectItemCaseSensitive(tone, "content");
    const cJSON *petal;
    bool roles = false;
    char *joined = NULL;
    size_t joined_len = 0;

    if (!cJSON_IsArray(content) || cJSON_GetArraySize(content) == 0) {
        return text && text[0] ? add_message(t, NULL, text) : -1;
    }

    cJSON_ArrayForEach(petal, content) {
        if (str_of(petal, "role")) roles = true;
    }
    cJSON_ArrayForEach(petal, content) {
        const char *kind = str_of(petal, "kind");
        const char *piece = str_of(petal, "text");
        if (!piece || !kind || strcmp(kind, THRUM_PETAL_TEXT) != 0) continue;
        if (roles) {
            if (add_message(t, str_of(petal, "role"), piece) != 0) return -1;
            continue;
        }
        {
            size_t n = strlen(piece);
            char *grown = (char *)realloc(joined, joined_len + n + 2);
            if (!grown) {
                free(joined);
                return -1;
            }
            joined = grown;
            if (joined_len > 0) joined[joined_len++] = '\n';
            memcpy(joined + joined_len, piece, n + 1);
            joined_len += n;
        }
    }
    if (joined) {
        int rc = joined_len > 0 ? add_message(t, NULL, joined) : -1;
        free(joined);
        return rc;
    }
    return t->count > 0 ? 0 : -1;
}

static void on_prompt(const cJSON *tone) {
    const char *model = str_of(tone, "modelId");
    const char *path = ds4_models_path(&g_models, model);
    struct bloom *t = (struct bloom *)calloc(1, sizeof *t);

    if (!t) return;
    t->sid = strdup(str_of(tone, "sid") ? str_of(tone, "sid") : "");
    t->rid = dup_of(str_of(tone, "rid"));
    if (!path) {
        char why[256];
        snprintf(why, sizeof why, "no DwarfStar model of this bee answers to %s",
                 model ? model : "the default");
        send_error(t, "no_such_model", why);
        free_bloom(t);
        return;
    }
    if (read_messages(tone, t) != 0) {
        send_error(t, "empty_prompt", "the prompt carried no text");
        free_bloom(t);
        return;
    }
    t->model_path = strdup(path);
    t->system = dup_of(str_of(tone, "systemPrompt"));

    pthread_mutex_lock(&g_lock);
    if (g_tail) {
        g_tail->next = t;
    } else {
        g_head = t;
    }
    g_tail = t;
    pthread_cond_signal(&g_ready);
    pthread_mutex_unlock(&g_lock);
}

static void on_cancel(const cJSON *tone) {
    const char *sid = str_of(tone, "sid");
    struct bloom *dropped = NULL;
    struct bloom **at;

    if (!sid) return;
    pthread_mutex_lock(&g_lock);
    if (g_current_sid && strcmp(g_current_sid, sid) == 0) ds4_bridge_cancel(g_bridge);
    at = &g_head;
    g_tail = NULL;
    while (*at) {
        struct bloom *t = *at;
        if (strcmp(t->sid, sid) == 0) {
            *at = t->next;
            t->next = dropped;
            dropped = t;
        } else {
            g_tail = t;
            at = &t->next;
        }
    }
    pthread_mutex_unlock(&g_lock);

    while (dropped) {
        struct bloom *t = dropped;
        dropped = t->next;
        send_wilt(t, "cancelled", 0, NULL);
        free_bloom(t);
    }
}

static void on_tone(thrum_bee *bee, const cJSON *tone, void *user) {
    const char *chi = str_of(tone, "chi");
    (void)bee;
    (void)user;
    if (!chi) return;
    if (strcmp(chi, THRUM_CHI_PROMPT) == 0) {
        on_prompt(tone);
    } else if (strcmp(chi, THRUM_CHI_CANCEL) == 0) {
        on_cancel(tone);
    }
}

static int metal_dir(char *out, size_t cap) {
    const char *env = env_of("DWARFSTAR_METAL_DIR");
    char path[2048];
    char *slash;

    if (env) {
        snprintf(out, cap, "%s", env);
        return 0;
    }
#if defined(__APPLE__)
    {
        uint32_t size = (uint32_t)sizeof path;
        if (_NSGetExecutablePath(path, &size) != 0) return -1;
    }
#else
    {
        ssize_t n = readlink("/proc/self/exe", path, sizeof path - 1);
        if (n <= 0) return -1;
        path[n] = '\0';
    }
#endif
    slash = strrchr(path, '/');
    if (!slash) return -1;
    *slash = '\0';
    snprintf(out, cap, "%s/metal", path);
    return 0;
}

static cJSON *hello_for(void) {
    static const char *const roles[] = {"worker"};
    static const char *const chis[] = {THRUM_CHI_HELLO, THRUM_CHI_PROMPT, THRUM_CHI_CANCEL,
                                       THRUM_CHI_UNFURL, THRUM_CHI_WILT, THRUM_CHI_ERROR,
                                       THRUM_CHI_PULSE};
    cJSON *hello = cJSON_CreateObject();
    cJSON *propensity;

    if (!hello) return NULL;
    cJSON_AddItemToObject(hello, "roles", cJSON_CreateStringArray(roles, 1));
    cJSON_AddStringToObject(hello, "version", VERSION);
    cJSON_AddItemToObject(hello, "models", ds4_models_ids(&g_models));
    cJSON_AddItemToObject(hello, "chis",
                          cJSON_CreateStringArray(chis, (int)(sizeof chis / sizeof chis[0])));
    propensity = cJSON_AddObjectToObject(hello, "propensity");
    cJSON_AddStringToObject(propensity, "statefulness", "stateless");
    cJSON_AddStringToObject(propensity, "richness", "lean");
    cJSON_AddStringToObject(propensity, "wire", WIRE);
    cJSON_AddItemToObject(hello, "headroom", headroom_of(atomic_load(&g_free_slots)));
    cJSON_AddStringToObject(hello, "source", SOURCE);
    return hello;
}

static void on_nestled(thrum_bee *bee, void *user) {
    (void)user;
    fprintf(stderr, "dwarfstar: %s nestled as %s with %d model%s\n", g_label, thrum_bee_id(bee),
            g_models.count, g_models.count == 1 ? "" : "s");
}

static void on_astray(thrum_bee *bee, void *user) {
    struct bloom *dropped;
    (void)bee;
    (void)user;
    pthread_mutex_lock(&g_lock);
    if (g_current_sid) ds4_bridge_cancel(g_bridge);
    dropped = g_head;
    g_head = NULL;
    g_tail = NULL;
    pthread_mutex_unlock(&g_lock);
    while (dropped) {
        struct bloom *t = dropped;
        dropped = t->next;
        free_bloom(t);
    }
    fprintf(stderr, "dwarfstar: humd went away; its blooms are dropped and the link is dialing again\n");
}

static void resolve(const char *dir, const char *path, char *out, size_t cap) {
    if (path[0] == '/') {
        snprintf(out, cap, "%s", path);
    } else {
        snprintf(out, cap, "%s/%s", dir, path);
    }
}

static int find_models(const char *dir) {
    const char *pinned = env_of("DWARFSTAR_MODEL");
    char path[1024];

    if (!pinned) {
        if (ds4_models_scan(&g_models, dir) <= 0) {
            fprintf(stderr, "dwarfstar: no DwarfStar models under %s; set DWARFSTAR_DIR\n", dir);
        }
        return 0;
    }
    resolve(dir, pinned, path, sizeof path);
    if (ds4_models_pin(&g_models, path, env_of("DWARFSTAR_MODELS")) <= 0) {
        fprintf(stderr, "dwarfstar: no model at %s to serve; this unit stays down\n", path);
        return 1;
    }
    return 0;
}

static int gpu_lock_path(char *out, size_t cap) {
    const char *env = env_of("DWARFSTAR_GPU_LOCK");
    if (!env) return -1;
    snprintf(out, cap, "%s", env);
    return 0;
}

int main(void) {
    const char *label = env_of("DWARFSTAR_LABEL") ? env_of("DWARFSTAR_LABEL") : "main";
    ds4_bridge_options options = {32768, 0, false};
    const char *env;
    char dir[1024];
    char kernels[2048] = "";
    char lock[1100];
    char err[512] = "DWARFSTAR_GPU_LOCK is unset; the Orchfile sets it under ${HUM_HIVE_STATE}";
    pthread_t worker;
    int rc;

    if ((env = env_of("DWARFSTAR_DIR"))) {
        snprintf(dir, sizeof dir, "%s", env);
    } else {
        snprintf(dir, sizeof dir, "%s/ds4", env_of("HOME") ? env_of("HOME") : ".");
    }
    if ((env = env_of("DWARFSTAR_CTX"))) options.ctx_size = atoi(env);
    if ((env = env_of("DWARFSTAR_THREADS"))) options.threads = atoi(env);
    if ((env = env_of("DWARFSTAR_SSD_STREAMING"))) options.ssd_streaming = strcmp(env, "0") != 0;
    if ((env = env_of("DWARFSTAR_MAX_TOKENS"))) g_max_tokens = atoi(env);
    if ((env = env_of("DWARFSTAR_TEMPERATURE"))) g_temperature = (float)atof(env);
    if ((env = env_of("DWARFSTAR_IDLE_MS"))) g_idle_ms = atoll(env);
    if ((env = env_of("DWARFSTAR_WAIT_MS"))) g_wait_ms = atoll(env);

    if (find_models(dir) != 0) return 0;
    if (metal_dir(kernels, sizeof kernels) != 0 || dwarfstar_shaders_point_at(kernels) != 0) {
        fprintf(stderr, "dwarfstar: the Metal kernels are not under %s\n", kernels);
        return 1;
    }
    if (gpu_lock_path(lock, sizeof lock) != 0 || !(g_gpu = gpu_open(lock, err, sizeof err))) {
        fprintf(stderr, "dwarfstar: %s\n", err);
        return 1;
    }
    g_bridge = ds4_bridge_new(&options);
    if (!g_bridge) {
        fprintf(stderr, "dwarfstar: the engine bridge could not start\n");
        return 1;
    }
    g_label = label;
    atomic_store(&g_free_slots, free_slots_now());
    {
        cJSON *hello = hello_for();
        thrum_options opts = {.hive = HIVE, .as = label, .hello = hello};
        g_bee = thrum_bee_new(&opts);
        cJSON_Delete(hello);
    }
    if (!g_bee) {
        fprintf(stderr, "dwarfstar: out of memory for the thrum client\n");
        ds4_bridge_free(g_bridge);
        gpu_close(g_gpu);
        return 1;
    }
    thrum_bee_on_any(g_bee, on_tone, NULL);
    thrum_bee_on_nestled(g_bee, on_nestled, NULL);
    thrum_bee_on_astray(g_bee, on_astray, NULL);
    g_last_bloom_ms = now_ms();
    if (pthread_create(&worker, NULL, take_blooms, NULL) != 0) {
        fprintf(stderr, "dwarfstar: could not start the bloom thread\n");
        return 1;
    }

    rc = thrum_bee_start(g_bee);
    if (rc == 0) rc = thrum_bee_wait(g_bee);
    if (rc != THRUM_LINK_CLOSED) fprintf(stderr, "dwarfstar: %s\n", thrum_bee_error(g_bee));

    pthread_mutex_lock(&g_lock);
    g_closing = true;
    ds4_bridge_cancel(g_bridge);
    pthread_cond_broadcast(&g_ready);
    pthread_mutex_unlock(&g_lock);
    pthread_join(worker, NULL);

    while (g_head) {
        struct bloom *t = g_head;
        g_head = t->next;
        free_bloom(t);
    }
    thrum_bee_free(g_bee);
    ds4_bridge_free(g_bridge);
    gpu_close(g_gpu);
    return rc == THRUM_LINK_REFUSED ? 3 : rc == THRUM_LINK_CLOSED ? 0 : 1;
}
