/* dwarfstar-bee: the DwarfStar engine as a hum worker.
 *
 * The bee enters itself under the dwarfstar hive, signs its hello through
 * the thrum C client, and answers prompts from the engine linked into this
 * process: a prompt tone becomes a chat prompt, the chat prompt becomes
 * tokens, and the tokens come back as text_delta chunks and a finish. No
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
 * The engine runs one turn at a time, so prompts queue and one thread
 * takes them in order and tends the GPU between them, while the main
 * thread reads the socket. A cancel stops its sid's turn, waiting or in
 * flight, and drops its sid's queued turns. The bee is stateless: every
 * prompt carries what it needs, as a content string (the way hum-nest
 * reads it and openai-server sends it), else text, else content petals,
 * and the engine's session reuses whatever of it the KV cache already
 * holds. A chunk always carries whole UTF-8 characters, even when a token
 * ends inside one.
 *
 * Environment:
 *   DWARFSTAR_MODEL        pin to this GGUF, absolute or under DWARFSTAR_DIR
 *   DWARFSTAR_MODELS       the ids a pinned bee answers to (default: by name)
 *   DWARFSTAR_LABEL        the label the bee enters as (default main)
 *   DWARFSTAR_DIR          where the GGUFs live (default ~/ds4)
 *   DWARFSTAR_CTX          context size in tokens (default 32768)
 *   DWARFSTAR_MAX_TOKENS   cap on one turn (default 4096)
 *   DWARFSTAR_TEMPERATURE  sampling temperature (default ds4's)
 *   DWARFSTAR_THREADS      engine CPU threads (default ds4's)
 *   DWARFSTAR_SSD_STREAMING  1 to stream experts from SSD
 *   DWARFSTAR_IDLE_MS      give the GPU back after this long idle (default 300000)
 *   DWARFSTAR_WAIT_MS      how long a turn waits for the GPU (default 300000)
 *   DWARFSTAR_GPU_LOCK     the hive-wide GPU lock
 *                          (default $XDG_STATE_HOME/hum/hives/dwarfstar/gpu.lock)
 *   DWARFSTAR_METAL_DIR    the .metal kernels (default metal/ beside the bee)
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include <errno.h>
#include <pthread.h>
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

struct turn {
    char *sid;
    char *rid;
    char *model_path;
    char *system;
    ds4_message *messages;
    size_t count;
    struct turn *next;
};

struct stream {
    const struct turn *turn;
    char *held;
    size_t len;
    size_t cap;
};

static thrum_client *g_client;
static ds4_bridge *g_bridge;
static dwarfstar_gpu *g_gpu;
static ds4_models g_models;
static int g_max_tokens = 4096;
static float g_temperature = -1.0f;
static int64_t g_idle_ms = 300000;
static int64_t g_wait_ms = 300000;

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_ready = PTHREAD_COND_INITIALIZER;
static struct turn *g_head;
static struct turn *g_tail;
static const char *g_current_sid;
static bool g_closing;

static int g_free_slots = -1;
static int64_t g_last_turn_ms;

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

static void free_turn(struct turn *t) {
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
    if (thrum_client_send(g_client, tone) != 0) {
        fprintf(stderr, "dwarfstar: %s\n", thrum_error());
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
    bool changed = free_slots != g_free_slots;
    g_free_slots = free_slots;
    if (!changed && strcmp(kind, THRUM_PULSE_CELL_IDLE) == 0) return;
    tone = thrum_tone_new(THRUM_CHI_PULSE, NULL, NULL);
    if (!tone) return;
    cJSON_AddStringToObject(tone, "kind", kind);
    cJSON_AddItemToObject(tone, "headroom", headroom_of(free_slots));
    send_tone(tone);
}

static void send_error(const struct turn *t, const char *code, const char *message) {
    cJSON *tone = thrum_tone_new(THRUM_CHI_ERROR, t->rid, t->sid);
    if (!tone) return;
    cJSON_AddStringToObject(tone, "code", code);
    cJSON_AddStringToObject(tone, "message", message);
    send_tone(tone);
}

static void send_finish(const struct turn *t, const char *reason, int exit_code,
                        const ds4_turn *result) {
    cJSON *tone = thrum_tone_new(THRUM_CHI_FINISH, t->rid, t->sid);
    cJSON *usage;
    if (!tone) return;
    cJSON_AddStringToObject(tone, "finishReason", reason);
    cJSON_AddNumberToObject(tone, "exitCode", exit_code);
    usage = cJSON_AddObjectToObject(tone, "usage");
    if (usage) {
        cJSON_AddNumberToObject(usage, "input_tokens", result ? result->tokens_in : 0);
        cJSON_AddNumberToObject(usage, "output_tokens", result ? result->tokens_out : 0);
    }
    send_tone(tone);
}

static void send_chunk(const struct turn *t, const char *text, size_t len) {
    cJSON *tone = thrum_tone_new(THRUM_CHI_CHUNK, t->rid, t->sid);
    char *delta = (char *)malloc(len + 1);
    if (!tone || !delta) {
        cJSON_Delete(tone);
        free(delta);
        return;
    }
    memcpy(delta, text, len);
    delta[len] = '\0';
    cJSON_AddStringToObject(tone, "chunkType", "text_delta");
    cJSON_AddStringToObject(tone, "delta", delta);
    free(delta);
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
    send_chunk(s->turn, s->held, ready);
    memmove(s->held, s->held + ready, s->len - ready);
    s->len -= ready;
}

static bool turn_stopped(void *user) {
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

static int take_gpu(const struct turn *t) {
    char err[512] = "";
    int rc;

    if (ds4_bridge_ready(g_bridge, t->model_path)) return 0;
    rc = gpu_acquire(g_gpu, g_wait_ms, turn_stopped, NULL, err, sizeof err);
    if (rc == 1) {
        send_finish(t, "cancelled", 0, NULL);
        return -1;
    }
    if (rc != 0) {
        send_error(t, "gpu_busy", err);
        send_finish(t, "error", 1, NULL);
        return -1;
    }
    if (ds4_bridge_load(g_bridge, t->model_path, err, sizeof err) != 0) {
        gpu_release(g_gpu);
        claim(free_slots_now(), THRUM_PULSE_CELL_DIED);
        send_error(t, "engine_unavailable", err);
        send_finish(t, "error", 1, NULL);
        return -1;
    }
    fprintf(stderr, "dwarfstar: loaded %s\n", t->model_path);
    claim(1, THRUM_PULSE_CELL_SPAWNED);
    return 0;
}

static void run_turn(const struct turn *t) {
    struct stream s = {t, NULL, 0, 0};
    ds4_turn result;
    char err[512];
    int rc;

    if (take_gpu(t) != 0) return;
    rc = ds4_bridge_generate(g_bridge, t->system, t->messages, t->count, g_temperature,
                             g_max_tokens, on_piece, &s, &result, err, sizeof err);
    free(s.held);
    if (rc != 0) {
        send_error(t, "generation_failed", err[0] ? err : "the engine failed");
        send_finish(t, "error", 1, &result);
        return;
    }
    send_finish(t, result.finish, 0, &result);
}

static void tend_gpu(bool after_turn) {
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
    if (!queued && (want_age >= 0 || thrum_now_ms() - g_last_turn_ms >= g_idle_ms)) {
        fprintf(stderr, "dwarfstar: giving the GPU back (%s)\n",
                want_age >= 0 ? "another model wants it" : "idle");
        give_back_gpu();
    } else if (after_turn && want_age >= YIELD_AFTER_MS) {
        fprintf(stderr, "dwarfstar: giving the GPU back after a turn (a want waited %llds)\n",
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

static void *take_turns(void *arg) {
    (void)arg;
    for (;;) {
        struct turn *t;
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

        run_turn(t);

        pthread_mutex_lock(&g_lock);
        g_current_sid = NULL;
        pthread_mutex_unlock(&g_lock);
        g_last_turn_ms = thrum_now_ms();
        free_turn(t);
        tend_gpu(true);
    }
    ds4_bridge_unload(g_bridge);
    gpu_release(g_gpu);
    return NULL;
}

static int add_message(struct turn *t, const char *role, const char *text) {
    ds4_message *grown = (ds4_message *)realloc(t->messages, (t->count + 1) * sizeof *grown);
    if (!grown) return -1;
    t->messages = grown;
    t->messages[t->count].role = dup_of(role);
    t->messages[t->count].text = dup_of(text);
    t->count++;
    return 0;
}

static int read_messages(const cJSON *tone, struct turn *t) {
    const char *text = str_of(tone, "text");
    const cJSON *content = cJSON_GetObjectItemCaseSensitive(tone, "content");
    const cJSON *petal;
    bool roles = false;
    char *joined = NULL;
    size_t joined_len = 0;

    if (cJSON_IsString(content) && content->valuestring[0]) {
        return add_message(t, NULL, content->valuestring);
    }
    if (text && text[0]) return add_message(t, NULL, text);
    if (!cJSON_IsArray(content)) return -1;

    cJSON_ArrayForEach(petal, content) {
        if (str_of(petal, "role")) roles = true;
    }
    cJSON_ArrayForEach(petal, content) {
        const char *type = str_of(petal, "type");
        const char *piece = str_of(petal, "text");
        if (!piece || (type && strcmp(type, "text") != 0)) continue;
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
    struct turn *t = (struct turn *)calloc(1, sizeof *t);

    if (!t) return;
    t->sid = strdup(str_of(tone, "sid") ? str_of(tone, "sid") : "");
    t->rid = dup_of(str_of(tone, "rid"));
    if (!path) {
        char why[256];
        snprintf(why, sizeof why, "no DwarfStar model of this bee answers to %s",
                 model ? model : "the default");
        send_error(t, "no_such_model", why);
        free_turn(t);
        return;
    }
    if (read_messages(tone, t) != 0) {
        send_error(t, "empty_prompt", "the prompt carried no text");
        free_turn(t);
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
    struct turn *dropped = NULL;
    struct turn **at;

    if (!sid) return;
    pthread_mutex_lock(&g_lock);
    if (g_current_sid && strcmp(g_current_sid, sid) == 0) ds4_bridge_cancel(g_bridge);
    at = &g_head;
    g_tail = NULL;
    while (*at) {
        struct turn *t = *at;
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
        struct turn *t = dropped;
        dropped = t->next;
        send_finish(t, "cancelled", 0, NULL);
        free_turn(t);
    }
}

static void on_tone(const cJSON *tone, void *user) {
    const char *chi = str_of(tone, "chi");
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
                                       THRUM_CHI_CHUNK, THRUM_CHI_FINISH, THRUM_CHI_ERROR,
                                       THRUM_CHI_PULSE};
    cJSON *hello = cJSON_CreateObject();
    cJSON *propensity;

    if (!hello) return NULL;
    cJSON_AddStringToObject(hello, "hive", HIVE);
    cJSON_AddItemToObject(hello, "roles", cJSON_CreateStringArray(roles, 1));
    cJSON_AddStringToObject(hello, "version", VERSION);
    cJSON_AddItemToObject(hello, "models", ds4_models_ids(&g_models));
    cJSON_AddItemToObject(hello, "chis",
                          cJSON_CreateStringArray(chis, (int)(sizeof chis / sizeof chis[0])));
    propensity = cJSON_AddObjectToObject(hello, "propensity");
    cJSON_AddStringToObject(propensity, "statefulness", "stateless");
    cJSON_AddStringToObject(propensity, "richness", "lean");
    cJSON_AddStringToObject(propensity, "wire", WIRE);
    g_free_slots = free_slots_now();
    cJSON_AddItemToObject(hello, "headroom", headroom_of(g_free_slots));
    cJSON_AddStringToObject(hello, "source", SOURCE);
    return hello;
}

static int nestle(const char *label) {
    thrum_bee bee;
    cJSON *hello;
    int rc;

    if (thrum_bee_enter_as(HIVE, label, &bee) != 0) return -1;
    g_client = thrum_client_new(NULL);
    if (!g_client || thrum_client_connect(g_client) != 0) {
        thrum_bee_forget(&bee);
        return -1;
    }
    hello = hello_for();
    rc = thrum_client_hello(g_client, &bee, hello, NULL);
    cJSON_Delete(hello);
    if (rc == 0) {
        fprintf(stderr, "dwarfstar: %s nestled as %s with %d model%s\n", label, bee.id,
                g_models.count, g_models.count == 1 ? "" : "s");
    }
    thrum_bee_forget(&bee);
    return rc;
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
    char state[1024];
    if (env) {
        snprintf(out, cap, "%s", env);
        return 0;
    }
    if (thrum_state_dir(state, sizeof state) != 0) return -1;
    snprintf(out, cap, "%s/hives/%s/gpu.lock", state, HIVE);
    return 0;
}

int main(void) {
    const char *label = env_of("DWARFSTAR_LABEL") ? env_of("DWARFSTAR_LABEL") : "main";
    ds4_bridge_options options = {32768, 0, false};
    const char *env;
    char dir[1024];
    char kernels[2048] = "";
    char lock[1100];
    char err[512] = "cannot find the state directory";
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
    if (nestle(label) != 0) {
        fprintf(stderr, "dwarfstar: %s\n", thrum_error());
        thrum_client_free(g_client);
        ds4_bridge_free(g_bridge);
        gpu_close(g_gpu);
        return 1;
    }
    g_last_turn_ms = thrum_now_ms();
    if (pthread_create(&worker, NULL, take_turns, NULL) != 0) {
        fprintf(stderr, "dwarfstar: could not start the turn thread\n");
        return 1;
    }

    thrum_client_on_any(g_client, on_tone, NULL);
    rc = thrum_client_run(g_client);
    if (rc < 0) fprintf(stderr, "dwarfstar: %s\n", thrum_error());

    pthread_mutex_lock(&g_lock);
    g_closing = true;
    ds4_bridge_cancel(g_bridge);
    pthread_cond_broadcast(&g_ready);
    pthread_mutex_unlock(&g_lock);
    pthread_join(worker, NULL);

    while (g_head) {
        struct turn *t = g_head;
        g_head = t->next;
        free_turn(t);
    }
    thrum_client_free(g_client);
    ds4_bridge_free(g_bridge);
    gpu_close(g_gpu);
    return rc < 0 ? 1 : 0;
}
