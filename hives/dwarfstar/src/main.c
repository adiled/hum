#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
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

#include "chat.h"
#include "ds4_bridge.h"
#include "gpu.h"
#include "models.h"
#include "shaders.h"

#define HIVE "dwarfstar"
#define VERSION "0.35.2"
#define SOURCE "https://github.com/adiled/hum/tree/main/hives/dwarfstar"
#define WIRE "dwarfstar/core"
#define TICK_MS 500
#define YIELD_AFTER_MS 60000

struct awaited {
    char *call_id;
    char *name;
    char *args_json;
    char *answer;
    bool is_error;
    bool answered;
};

struct bloom {
    char *sid;
    char *rid;
    char *model_path;
    char *system;
    chat said;
    cJSON *tools;
    bool tendrils_in_play;
    int next_petal;
    ds4_wilt nectar;
    struct awaited *awaited;
    size_t awaited_count;
    int64_t answers_due_by;
    struct bloom *next;
};

struct growth {
    const struct bloom *bloom;
    bool watching_for_tendrils;
    bool tendrils_began;
    char *grown;
    size_t len;
    size_t room;
    size_t shown;
    int *next_petal;
    int words_petal;
};

typedef enum {
    BLOOM_DONE,
    BLOOM_AWAITS_ANSWERS,
} bloom_fate;

typedef enum {
    GROWS_FROM_NEW_WORDS,
    GROWS_ON_FROM_ANSWERS,
    NOTHING_NEW_TO_GROW_FROM,
    NOTHING_AT_ALL,
} what_to_grow;

static thrum_bee *g_bee;
static ds4_bridge *g_bridge;
static dwarfstar_gpu *g_gpu;
static ds4_models g_models;
static int g_max_tokens = 4096;
static float g_temperature = -1.0f;
static int64_t g_idle_ms = 300000;
static int64_t g_wait_ms = 300000;
static int64_t g_reach_ms = 1800000;

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_ready = PTHREAD_COND_INITIALIZER;
static struct bloom *g_head;
static struct bloom *g_tail;
static const char *g_current_sid;
static bool g_closing;
static struct bloom *g_parked;

static atomic_int g_free_slots = -1;
static const char *g_label = "main";
static int64_t g_last_bloom_ms;

static const char *str_of(const cJSON *tone, const char *key) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static bool is_kind(const cJSON *petal, const char *kind) {
    const char *petal_kind = str_of(petal, "kind");
    return petal_kind && strcmp(petal_kind, kind) == 0;
}

static char *dup_of(const char *text) {
    return text ? strdup(text) : NULL;
}

static char *json_text_of(const cJSON *item) {
    char *printed = cJSON_PrintUnformatted(item);
    char *copy = dup_of(printed);
    cJSON_free(printed);
    return copy;
}

static char *answer_text_of(const cJSON *result) {
    if (cJSON_IsString(result)) return strdup(result->valuestring);
    return result ? json_text_of(result) : strdup("");
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

static struct timespec tick_from_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    ts.tv_nsec += (long)TICK_MS * 1000000L;
    ts.tv_sec += ts.tv_nsec / 1000000000L;
    ts.tv_nsec %= 1000000000L;
    return ts;
}

static void mint_call_id(char *out, size_t cap) {
    static atomic_uint minted;
    unsigned char noise[12] = {0};
    FILE *urandom = fopen("/dev/urandom", "rb");
    size_t got = urandom ? fread(noise, 1, sizeof noise, urandom) : 0;
    size_t i;
    if (urandom) fclose(urandom);
    if (got < sizeof noise) {
        uint64_t fallback = (uint64_t)now_ms() ^ ((uint64_t)getpid() << 32) ^ atomic_fetch_add(&minted, 1);
        for (i = 0; i < sizeof noise; i++) noise[i] = (unsigned char)(fallback >> ((i % 8) * 8));
    }
    snprintf(out, cap, "call_");
    for (i = 0; i < sizeof noise && strlen(out) + 2 < cap; i++) {
        snprintf(out + strlen(out), cap - strlen(out), "%02x", noise[i]);
    }
}

static cJSON *tone_new(const char *chi, const char *rid, const char *sid) {
    cJSON *tone = cJSON_CreateObject();
    if (!tone) return NULL;
    cJSON_AddStringToObject(tone, "chi", chi);
    if (rid) cJSON_AddStringToObject(tone, "rid", rid);
    if (sid) cJSON_AddStringToObject(tone, "sid", sid);
    return tone;
}

static void forget_awaited(struct bloom *t) {
    size_t i;
    for (i = 0; i < t->awaited_count; i++) {
        free(t->awaited[i].call_id);
        free(t->awaited[i].name);
        free(t->awaited[i].args_json);
        free(t->awaited[i].answer);
    }
    free(t->awaited);
    t->awaited = NULL;
    t->awaited_count = 0;
}

static void free_bloom(struct bloom *t) {
    if (!t) return;
    forget_awaited(t);
    chat_free(&t->said);
    cJSON_Delete(t->tools);
    free(t->sid);
    free(t->rid);
    free(t->model_path);
    free(t->system);
    free(t);
}

static void free_blooms(struct bloom *t) {
    while (t) {
        struct bloom *next = t->next;
        free_bloom(t);
        t = next;
    }
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

static cJSON *unfurl_tone(const struct bloom *t, int petal, const char *kind) {
    cJSON *tone = tone_new(THRUM_CHI_UNFURL, t->rid, t->sid);
    if (!tone) return NULL;
    cJSON_AddNumberToObject(tone, "petal", petal);
    cJSON_AddStringToObject(tone, "kind", kind);
    return tone;
}

static void unfurl_words(const struct bloom *t, int petal, const char *text, size_t len) {
    cJSON *tone = unfurl_tone(t, petal, THRUM_PETAL_TEXT);
    char *words = (char *)malloc(len + 1);
    if (!tone || !words) {
        cJSON_Delete(tone);
        free(words);
        return;
    }
    memcpy(words, text, len);
    words[len] = '\0';
    cJSON_AddStringToObject(tone, "text", words);
    free(words);
    send_tone(tone);
}

static cJSON *closed_petal_tone(const struct bloom *t, int petal, const char *kind) {
    cJSON *tone = unfurl_tone(t, petal, kind);
    if (tone) cJSON_AddBoolToObject(tone, "closed", 1);
    return tone;
}

static void close_petal(const struct bloom *t, int petal, const char *kind) {
    send_tone(closed_petal_tone(t, petal, kind));
}

static cJSON *args_of(const char *args_json) {
    cJSON *args = cJSON_Parse(args_json ? args_json : "");
    if (cJSON_IsObject(args)) return args;
    cJSON_Delete(args);
    return cJSON_CreateObject();
}

static cJSON *tendril_petal_tone(const struct bloom *t, int petal, const tendril *reached) {
    cJSON *tone = unfurl_tone(t, petal, THRUM_PETAL_TENDRIL);
    if (!tone) return NULL;
    cJSON_AddStringToObject(tone, "callId", reached->call_id);
    cJSON_AddStringToObject(tone, "name", reached->name);
    cJSON_AddStringToObject(tone, "sap", reached->args_json);
    return tone;
}

static cJSON *reach_tone(const struct bloom *t, const tendril *reached) {
    cJSON *tone = tone_new(THRUM_CHI_TENDRIL_REACH, NULL, t->sid);
    if (!tone) return NULL;
    cJSON_AddStringToObject(tone, "callId", reached->call_id);
    cJSON_AddStringToObject(tone, "name", reached->name);
    cJSON_AddItemToObject(tone, "args", args_of(reached->args_json));
    return tone;
}

static cJSON *what_came_back_tone(const struct bloom *t, const struct awaited *a) {
    cJSON *tone = tone_new(THRUM_CHI_TENDRIL_INFO, NULL, t->sid);
    if (!tone) return NULL;
    cJSON_AddStringToObject(tone, "callId", a->call_id);
    cJSON_AddStringToObject(tone, "name", a->name);
    cJSON_AddItemToObject(tone, "args", args_of(a->args_json));
    cJSON_AddStringToObject(tone, "result", a->answer ? a->answer : "");
    cJSON_AddBoolToObject(tone, "isError", a->is_error);
    return tone;
}

static void send_each(cJSON *tones) {
    cJSON *tone;
    while (tones && (tone = cJSON_DetachItemFromArray(tones, 0)) != NULL) send_tone(tone);
    cJSON_Delete(tones);
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

static struct growth growth_of(const struct bloom *t, bool watching_for_tendrils, int *next_petal) {
    struct growth g;
    memset(&g, 0, sizeof g);
    g.bloom = t;
    g.watching_for_tendrils = watching_for_tendrils;
    g.next_petal = next_petal;
    g.words_petal = -1;
    return g;
}

static bool grow_by(struct growth *g, const char *text, size_t len) {
    if (g->len + len + 1 > g->room) {
        size_t room = g->room ? g->room : 256;
        char *grown;
        while (room < g->len + len + 1) room *= 2;
        grown = (char *)realloc(g->grown, room);
        if (!grown) return false;
        g->grown = grown;
        g->room = room;
    }
    memcpy(g->grown + g->len, text, len);
    g->len += len;
    g->grown[g->len] = '\0';
    return true;
}

static void show_words_up_to(struct growth *g, size_t safe) {
    size_t ready;
    if (safe <= g->shown) return;
    ready = complete_utf8((const unsigned char *)g->grown, safe);
    if (ready <= g->shown) return;
    if (g->words_petal < 0) g->words_petal = (*g->next_petal)++;
    unfurl_words(g->bloom, g->words_petal, g->grown + g->shown, ready - g->shown);
    g->shown = ready;
}

static void on_piece(void *user, const char *text, size_t len) {
    struct growth *g = (struct growth *)user;
    if (!grow_by(g, text, len)) return;
    show_words_up_to(g, g->watching_for_tendrils ? words_safe_to_show(g->grown, &g->tendrils_began)
                                                 : g->len);
}

static void close_words(struct growth *g) {
    if (g->words_petal >= 0) close_petal(g->bloom, g->words_petal, THRUM_PETAL_TEXT);
}

static void show_every_word_left(struct growth *g) {
    show_words_up_to(g, g->len);
    close_words(g);
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
        send_wilt(t, "cancel", 0, NULL);
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

static ds4_message *messages_of(const chat *said, size_t *count) {
    ds4_message *messages = (ds4_message *)calloc(said->count ? said->count : 1, sizeof *messages);
    size_t i;
    *count = 0;
    if (!messages) return NULL;
    for (i = 0; i < said->count; i++) {
        const turn *spoken = &said->turns[i];
        if (spoken->by == TOOL_ANSWERS || !spoken->words) continue;
        messages[*count].role = spoken->by == USER_SAYS ? "user" : "assistant";
        messages[*count].text = spoken->words;
        (*count)++;
    }
    return messages;
}

static void grow_words_only(struct bloom *t) {
    int next_petal = 0;
    struct growth g = growth_of(t, false, &next_petal);
    size_t count;
    ds4_message *messages = messages_of(&t->said, &count);
    ds4_wilt result;
    char err[512];
    int rc;

    if (!messages) {
        send_error(t, "generation_failed", "out of memory for the prompt");
        send_wilt(t, "error", 1, NULL);
        return;
    }
    rc = ds4_bridge_generate(g_bridge, t->system, messages, count, g_temperature, g_max_tokens,
                             on_piece, &g, &result, err, sizeof err);
    free(messages);
    show_every_word_left(&g);
    free(g.grown);
    if (rc != 0) {
        send_error(t, "generation_failed", err[0] ? err : "the engine failed");
        send_wilt(t, "error", 1, &result);
        return;
    }
    send_wilt(t, result.reason, 0, &result);
}

static bool await_every_tendril(struct bloom *t, const reaching *r) {
    size_t i;
    t->awaited = (struct awaited *)calloc(r->count, sizeof *t->awaited);
    if (!t->awaited) return false;
    t->awaited_count = r->count;
    for (i = 0; i < r->count; i++) {
        t->awaited[i].call_id = dup_of(r->tendrils[i].call_id);
        t->awaited[i].name = dup_of(r->tendrils[i].name);
        t->awaited[i].args_json = dup_of(r->tendrils[i].args_json);
        if (!t->awaited[i].call_id || !t->awaited[i].name || !t->awaited[i].args_json) return false;
    }
    t->answers_due_by = now_ms() + g_reach_ms;
    return true;
}

static bool every_answer_came(const struct bloom *t) {
    size_t i;
    for (i = 0; i < t->awaited_count; i++) {
        if (!t->awaited[i].answered) return false;
    }
    return true;
}

static bool remember_reaching(struct bloom *t, const struct growth *g, reaching *r) {
    turn *spoken = chat_add_turn(&t->said, MODEL_SAYS);
    size_t i;
    if (!spoken) return false;
    spoken->words = (char *)malloc(r->words_end + 1);
    spoken->tendrils_as_sampled = (char *)malloc(r->tendrils_end - r->tendrils_start + 1);
    if (!spoken->words || !spoken->tendrils_as_sampled) return false;
    memcpy(spoken->words, g->grown, r->words_end);
    spoken->words[r->words_end] = '\0';
    memcpy(spoken->tendrils_as_sampled, g->grown + r->tendrils_start, r->tendrils_end - r->tendrils_start);
    spoken->tendrils_as_sampled[r->tendrils_end - r->tendrils_start] = '\0';
    for (i = 0; i < r->count; i++) {
        char call_id[64];
        mint_call_id(call_id, sizeof call_id);
        r->tendrils[i].call_id = strdup(call_id);
        if (!r->tendrils[i].call_id ||
            turn_add_tendril(spoken, call_id, r->tendrils[i].name, r->tendrils[i].args_json) != 0) {
            return false;
        }
    }
    return true;
}

static bool remember_answers(struct bloom *t) {
    size_t i;
    for (i = 0; i < t->awaited_count; i++) {
        turn *answer = chat_add_turn(&t->said, TOOL_ANSWERS);
        if (!answer || turn_add_words(answer, t->awaited[i].answer ? t->awaited[i].answer : "", NULL) != 0) {
            return false;
        }
        answer->answering_call_id = dup_of(t->awaited[i].call_id);
    }
    forget_awaited(t);
    return true;
}

static void add_to_nectar(ds4_wilt *total, const ds4_wilt *round) {
    total->tokens_in += round->tokens_in;
    total->tokens_out += round->tokens_out;
    total->reason = round->reason;
}

static void break_off(const struct bloom *t, const char *code, const char *message) {
    send_error(t, code, message);
    send_wilt(t, "error", 1, &t->nectar);
}

static cJSON *tones_reaching_for(struct bloom *t, const reaching *r) {
    cJSON *tones = cJSON_CreateArray();
    size_t i;
    if (!tones) return NULL;
    for (i = 0; i < r->count; i++) {
        int petal = t->next_petal++;
        cJSON_AddItemToArray(tones, tendril_petal_tone(t, petal, &r->tendrils[i]));
        cJSON_AddItemToArray(tones, closed_petal_tone(t, petal, THRUM_PETAL_TENDRIL));
    }
    for (i = 0; i < r->count; i++) cJSON_AddItemToArray(tones, reach_tone(t, &r->tendrils[i]));
    return tones;
}

static void park(struct bloom *t) {
    pthread_mutex_lock(&g_lock);
    t->next = g_parked;
    g_parked = t;
    g_current_sid = NULL;
    pthread_mutex_unlock(&g_lock);
}

static struct bloom *unpark_where(bool (*fits)(const struct bloom *, const void *), const void *what) {
    struct bloom *taken = NULL;
    struct bloom **at = &g_parked;
    while (*at) {
        struct bloom *t = *at;
        if (fits(t, what)) {
            *at = t->next;
            t->next = taken;
            taken = t;
        } else {
            at = &t->next;
        }
    }
    return taken;
}

static bool of_sid(const struct bloom *t, const void *sid) {
    return strcmp(t->sid, (const char *)sid) == 0;
}

static bool overdue(const struct bloom *t, const void *now) {
    return *(const int64_t *)now >= t->answers_due_by;
}

static bool answered_whole(const struct bloom *t, const void *sid) {
    return of_sid(t, sid) && every_answer_came(t);
}

static void queue_first(struct bloom *t) {
    t->next = g_head;
    g_head = t;
    if (!g_tail) g_tail = t;
    pthread_cond_signal(&g_ready);
}

static void break_off_overdue(void) {
    int64_t now = now_ms();
    struct bloom *late;
    pthread_mutex_lock(&g_lock);
    late = unpark_where(overdue, &now);
    pthread_mutex_unlock(&g_lock);
    for (struct bloom *t = late; t; t = t->next) {
        char why[512];
        snprintf(why, sizeof why, "%s did not come back within %llds",
                 t->awaited_count ? t->awaited[0].name : "a tendril", (long long)(g_reach_ms / 1000));
        fprintf(stderr, "dwarfstar: a bloom on %s let go: %s\n", t->sid, why);
        break_off(t, "tendril_unanswered", why);
    }
    free_blooms(late);
}

static bloom_fate grow_a_round(struct bloom *t) {
    dialect d = ds4_bridge_dialect(g_bridge);
    struct growth g = growth_of(t, true, &t->next_petal);
    char *prompt;
    reaching r;
    ds4_wilt round;
    char err[512];
    cJSON *tones;
    int rc;

    if (!remember_answers(t) || !(prompt = chat_as_prompt(d, t->system, t->tools, &t->said))) {
        break_off(t, "generation_failed", "out of memory for the prompt");
        return BLOOM_DONE;
    }
    rc = ds4_bridge_generate_from_prompt(g_bridge, prompt, g_temperature,
                                         g_max_tokens - t->nectar.tokens_out, on_piece, &g, &round,
                                         err, sizeof err);
    free(prompt);
    add_to_nectar(&t->nectar, &round);
    if (rc != 0) {
        show_every_word_left(&g);
        free(g.grown);
        break_off(t, "generation_failed", err[0] ? err : "the engine failed");
        return BLOOM_DONE;
    }
    if (strcmp(round.reason, "stop") != 0 || !read_tendrils(d, g.grown ? g.grown : "", t->tools, &r)) {
        show_every_word_left(&g);
        free(g.grown);
        send_wilt(t, t->nectar.reason, 0, &t->nectar);
        return BLOOM_DONE;
    }
    close_words(&g);
    if (!remember_reaching(t, &g, &r) || !await_every_tendril(t, &r) ||
        !(tones = tones_reaching_for(t, &r))) {
        reaching_free(&r);
        free(g.grown);
        break_off(t, "generation_failed", "out of memory for the tendrils");
        return BLOOM_DONE;
    }
    reaching_free(&r);
    free(g.grown);
    park(t);
    send_each(tones);
    return BLOOM_AWAITS_ANSWERS;
}

static bloom_fate run_bloom(struct bloom *t) {
    if (take_gpu(t) != 0) return BLOOM_DONE;
    if (t->tendrils_in_play) return grow_a_round(t);
    grow_words_only(t);
    return BLOOM_DONE;
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
                break_off_overdue();
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
        t->next = NULL;
        g_current_sid = t->sid;
        ds4_bridge_arm(g_bridge);
        pthread_mutex_unlock(&g_lock);

        if (run_bloom(t) == BLOOM_DONE) {
            pthread_mutex_lock(&g_lock);
            g_current_sid = NULL;
            pthread_mutex_unlock(&g_lock);
            free_bloom(t);
        }
        g_last_bloom_ms = now_ms();
        break_off_overdue();
        tend_gpu(true);
    }
    ds4_bridge_unload(g_bridge);
    gpu_release(g_gpu);
    return NULL;
}

static bool began_here(const chat *said, size_t first_of_these) {
    return said->count > first_of_these;
}

static turn *turn_for_words(chat *said, speaker by, size_t first_of_these) {
    turn *last = chat_last_turn(said);
    if (last && last->by == by && began_here(said, first_of_these)) return last;
    return chat_add_turn(said, by);
}

static turn *turn_for_tendril(chat *said, size_t first_of_these) {
    turn *last = chat_last_turn(said);
    if (last && last->by == MODEL_SAYS && began_here(said, first_of_these)) return last;
    return chat_add_turn(said, MODEL_SAYS);
}

static int hear_words(chat *said, speaker by, const char *words, size_t first_of_these) {
    turn *speaking = turn_for_words(said, by, first_of_these);
    return speaking ? turn_add_words(speaking, words, "\n") : -1;
}

static int hear_tendril(chat *said, const cJSON *petal, size_t first_of_these) {
    turn *speaking = turn_for_tendril(said, first_of_these);
    char *args_json = json_text_of(cJSON_GetObjectItemCaseSensitive(petal, "args"));
    int rc = speaking ? turn_add_tendril(speaking, str_of(petal, "callId"), str_of(petal, "name"),
                                         args_json && strcmp(args_json, "null") != 0 ? args_json : "{}")
                      : -1;
    free(args_json);
    return rc;
}

static int hear_answer(chat *said, const cJSON *petal) {
    turn *answer = chat_add_turn(said, TOOL_ANSWERS);
    char *words = answer_text_of(cJSON_GetObjectItemCaseSensitive(petal, "result"));
    int rc = answer && words ? turn_add_words(answer, words, NULL) : -1;
    if (rc == 0) answer->answering_call_id = dup_of(str_of(petal, "callId"));
    free(words);
    return rc;
}

static int hear_petals(struct bloom *t, const cJSON *petals, speaker words_by) {
    const size_t first_of_these = t->said.count;
    const cJSON *petal;
    cJSON_ArrayForEach(petal, petals) {
        int rc = 0;
        if (is_kind(petal, THRUM_PETAL_TEXT) && str_of(petal, "text")) {
            rc = hear_words(&t->said, words_by, str_of(petal, "text"), first_of_these);
        } else if (is_kind(petal, THRUM_PETAL_TENDRIL)) {
            rc = hear_tendril(&t->said, petal, first_of_these);
            t->tendrils_in_play = true;
        } else if (is_kind(petal, THRUM_PETAL_GRASP)) {
            rc = hear_answer(&t->said, petal);
            t->tendrils_in_play = true;
        }
        if (rc != 0) return -1;
    }
    return 0;
}

static bool said_something_new(const chat *said, size_t first_new) {
    size_t i;
    for (i = first_new; i < said->count; i++) {
        if (said->turns[i].by == USER_SAYS) return true;
    }
    return false;
}

static what_to_grow hear_the_chat(const cJSON *tone, struct bloom *t) {
    const cJSON *graft = cJSON_GetObjectItemCaseSensitive(tone, "graft");
    const cJSON *bloom;
    const char *text = str_of(tone, "text");
    const turn *last;
    size_t first_new;

    cJSON_ArrayForEach(bloom, graft) {
        if (hear_petals(t, cJSON_GetObjectItemCaseSensitive(bloom, "content"), USER_SAYS) != 0 ||
            hear_petals(t, cJSON_GetObjectItemCaseSensitive(bloom, "grew"), MODEL_SAYS) != 0) {
            return NOTHING_AT_ALL;
        }
    }
    first_new = t->said.count;
    if (hear_petals(t, cJSON_GetObjectItemCaseSensitive(tone, "content"), USER_SAYS) != 0) {
        return NOTHING_AT_ALL;
    }
    if (!said_something_new(&t->said, first_new) && text && text[0]) {
        turn *asked = chat_add_turn(&t->said, USER_SAYS);
        if (!asked || turn_add_words(asked, text, NULL) != 0) return NOTHING_AT_ALL;
    }
    last = chat_last_turn(&t->said);
    if (!last) return NOTHING_AT_ALL;
    if (said_something_new(&t->said, first_new)) return GROWS_FROM_NEW_WORDS;
    if (last->by == TOOL_ANSWERS) return GROWS_ON_FROM_ANSWERS;
    return NOTHING_NEW_TO_GROW_FROM;
}

static bool names_include(const cJSON *names, const char *name) {
    const cJSON *allowed;
    cJSON_ArrayForEach(allowed, names) {
        if (cJSON_IsString(allowed) && strcmp(allowed->valuestring, name) == 0) return true;
    }
    return false;
}

static cJSON *tools_offered(const cJSON *tone) {
    const cJSON *tools = cJSON_GetObjectItemCaseSensitive(tone, "tools");
    const cJSON *allowed = cJSON_GetObjectItemCaseSensitive(tone, "allowedTools");
    const char *reach_how = str_of(cJSON_GetObjectItemCaseSensitive(tone, "tendrils"), "reach");
    const cJSON *tool;
    cJSON *offered;

    if (!cJSON_IsArray(tools) || (reach_how && strcmp(reach_how, "none") == 0)) return NULL;
    offered = cJSON_CreateArray();
    if (!offered) return NULL;
    cJSON_ArrayForEach(tool, tools) {
        const char *name = str_of(tool, "name");
        if (!name || (cJSON_IsArray(allowed) && !names_include(allowed, name))) continue;
        cJSON_AddItemToArray(offered, cJSON_Duplicate(tool, true));
    }
    if (cJSON_GetArraySize(offered) > 0) return offered;
    cJSON_Delete(offered);
    return NULL;
}

static void on_prompt(const cJSON *tone) {
    const char *model = str_of(tone, "modelId");
    const char *path = ds4_models_path(&g_models, model);
    struct bloom *t = (struct bloom *)calloc(1, sizeof *t);
    struct bloom *superseded;

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
    t->tools = tools_offered(tone);
    t->tendrils_in_play = t->tools != NULL;
    switch (hear_the_chat(tone, t)) {
    case GROWS_FROM_NEW_WORDS:
    case GROWS_ON_FROM_ANSWERS:
        break;
    case NOTHING_NEW_TO_GROW_FROM:
        send_error(t, "cannot-grow-on", "dwarfstar grows a bloom only from new words, or on from what its tendrils grasped, and cannot grow one on from its graft's words");
        free_bloom(t);
        return;
    case NOTHING_AT_ALL:
        send_error(t, "empty-prompt", "the prompt carried no words");
        free_bloom(t);
        return;
    }
    t->model_path = strdup(path);
    t->system = dup_of(str_of(tone, "systemPrompt"));
    t->nectar.reason = "stop";

    pthread_mutex_lock(&g_lock);
    superseded = unpark_where(of_sid, t->sid);
    if (g_tail) {
        g_tail->next = t;
    } else {
        g_head = t;
    }
    g_tail = t;
    pthread_cond_signal(&g_ready);
    pthread_mutex_unlock(&g_lock);
    free_blooms(superseded);
}

static struct awaited *awaited_unanswered(struct bloom *t, const char *call_id) {
    size_t i;
    for (i = 0; i < t->awaited_count; i++) {
        if (!t->awaited[i].answered && strcmp(t->awaited[i].call_id, call_id) == 0) return &t->awaited[i];
    }
    return NULL;
}

static struct bloom *parked_of_sid(const char *sid) {
    struct bloom *t;
    for (t = g_parked; t; t = t->next) {
        if (strcmp(t->sid, sid) == 0) return t;
    }
    return NULL;
}

static void on_tendril_result(const cJSON *tone) {
    const char *sid = str_of(tone, "sid");
    const char *call_id = str_of(tone, "callId");
    const char *output = str_of(tone, "output");
    cJSON *what_came_back = NULL;
    struct bloom *t;
    struct awaited *a;
    bool resume = false;

    if (!sid || !call_id) return;
    pthread_mutex_lock(&g_lock);
    t = parked_of_sid(sid);
    a = t ? awaited_unanswered(t, call_id) : NULL;
    if (a) {
        a->answer = output ? strdup(output)
                           : answer_text_of(cJSON_GetObjectItemCaseSensitive(tone, "result"));
        a->is_error = cJSON_IsTrue(cJSON_GetObjectItemCaseSensitive(tone, "isError"));
        a->answered = true;
        what_came_back = what_came_back_tone(t, a);
        resume = every_answer_came(t);
    }
    pthread_mutex_unlock(&g_lock);
    send_tone(what_came_back);
    if (!resume) return;
    pthread_mutex_lock(&g_lock);
    t = unpark_where(answered_whole, sid);
    if (t) queue_first(t);
    pthread_mutex_unlock(&g_lock);
}

static void on_cancel(const cJSON *tone) {
    const char *sid = str_of(tone, "sid");
    struct bloom *dropped = NULL;
    struct bloom *parked;
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
    parked = unpark_where(of_sid, sid);
    pthread_mutex_unlock(&g_lock);

    for (struct bloom *t = parked; t; t = t->next) send_wilt(t, "cancel", 0, &t->nectar);
    free_blooms(parked);
    for (struct bloom *t = dropped; t; t = t->next) send_wilt(t, "cancel", 0, NULL);
    free_blooms(dropped);
}

static void on_tone(thrum_bee *bee, const cJSON *tone, void *user) {
    const char *chi = str_of(tone, "chi");
    (void)bee;
    (void)user;
    if (!chi) return;
    if (strcmp(chi, THRUM_CHI_PROMPT) == 0) {
        on_prompt(tone);
    } else if (strcmp(chi, THRUM_CHI_TENDRIL_RESULT) == 0) {
        on_tendril_result(tone);
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
    static const char *const chis[] = {THRUM_CHI_HELLO,          THRUM_CHI_PROMPT,
                                       THRUM_CHI_CANCEL,         THRUM_CHI_UNFURL,
                                       THRUM_CHI_WILT,           THRUM_CHI_ERROR,
                                       THRUM_CHI_PULSE,          THRUM_CHI_TENDRIL_REACH,
                                       THRUM_CHI_TENDRIL_RESULT, THRUM_CHI_TENDRIL_INFO};
    cJSON *hello = cJSON_CreateObject();
    cJSON *propensity;

    if (!hello) return NULL;
    cJSON_AddStringToObject(hello, "version", VERSION);
    cJSON_AddItemToObject(hello, "models", ds4_models_ids(&g_models));
    cJSON_AddItemToObject(hello, "chis",
                          cJSON_CreateStringArray(chis, (int)(sizeof chis / sizeof chis[0])));
    propensity = cJSON_AddObjectToObject(hello, "propensity");
    cJSON_AddStringToObject(propensity, "statefulness", "stateless");
    cJSON_AddStringToObject(propensity, "richness", "medium");
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
    struct bloom *parked;
    (void)bee;
    (void)user;
    pthread_mutex_lock(&g_lock);
    if (g_current_sid) ds4_bridge_cancel(g_bridge);
    dropped = g_head;
    parked = g_parked;
    g_head = NULL;
    g_tail = NULL;
    g_parked = NULL;
    pthread_mutex_unlock(&g_lock);
    free_blooms(dropped);
    free_blooms(parked);
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
    pthread_t blooms;
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
    if ((env = env_of("DWARFSTAR_REACH_MS"))) g_reach_ms = atoll(env);

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
        thrum_options opts = {.hive = HIVE, .as = label, .role = THRUM_ROLE_NURSE, .hello = hello};
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
    if (pthread_create(&blooms, NULL, take_blooms, NULL) != 0) {
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
    pthread_join(blooms, NULL);

    free_blooms(g_head);
    free_blooms(g_parked);
    thrum_bee_free(g_bee);
    ds4_bridge_free(g_bridge);
    gpu_close(g_gpu);
    return rc == THRUM_LINK_REFUSED ? 3 : rc == THRUM_LINK_CLOSED ? 0 : 1;
}
