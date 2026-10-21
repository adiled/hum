/* dwarfstar-bee — the DwarfStar bee.
 *
 * A bee is the process that dials into a humd. The hive is the kind it
 * conforms to: `dwarfstar`. This binary is the bee.
 *
 * It speaks thrum to humd and speaks ds4 to the engine. No HTTP in
 * between: the DwarfStar C library is linked into this process, so a
 * prompt goes from a tone straight into the model's token stream and the
 * answer comes back as chunk tones.
 *
 *   chi:"prompt"  ->  ds4 session  ->  chi:"chunk" ...  ->  chi:"finish"
 *
 * Environment:
 *   DWARFSTAR_MODEL        model file (default: ds4flash.gguf)
 *   DWARFSTAR_CTX          context size (default: 32768)
 *   DWARFSTAR_MAX_TOKENS   cap on one turn (default: 4096)
 *   DWARFSTAR_TEMPERATURE  sampling temperature (default: 1.0)
 *   DWARFSTAR_METAL_DIR    where the .metal kernels live (default: next
 *                          to this binary, in metal/)
 */

#define _POSIX_C_SOURCE 200809L

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

#include "thrum/client.h"
#include "thrum/identity.h"

#include "ds4_bridge.h"
#include "models.h"
#include "shaders.h"

#define HIVE_NAME "dwarfstar"
#define HIVE_VERSION "0.33.1"
#define HIVE_SOURCE "https://github.com/adiled/hum/tree/main/hives/dwarfstar"

static thrum_client g_client;
static thrum_identity g_identity;
static ds4_bridge *g_bridge;
static ds4_models g_models;
static char g_dir[1024] = "";
static int g_ctx = 32768;
static int g_max_tokens = 4096;
static float g_temperature = 1.0f;

typedef struct {
    char sid[160];
    char rid[160];
    char model_path[1024];
    char *prompt;
    char *system;
} turn;

static int executable_dir(char *out, size_t cap) {
#if defined(__APPLE__)
    char path[2048];
    uint32_t size = (uint32_t)sizeof path;
    char *slash;
    if (_NSGetExecutablePath(path, &size) != 0) return -1;
    slash = strrchr(path, '/');
    if (!slash) return -1;
    *slash = '\0';
    snprintf(out, cap, "%s", path);
    return 0;
#else
    ssize_t n = readlink("/proc/self/exe", out, cap - 1);
    char *slash;
    if (n <= 0) return -1;
    out[n] = '\0';
    slash = strrchr(out, '/');
    if (!slash) return -1;
    *slash = '\0';
    return 0;
#endif
}

static void send_chunk(const turn *t, const char *text, size_t len) {
    thrum_buf b;
    thrum_buf_init(&b);
    thrum_tone_begin(&b, CHI_CHUNK, t->rid, t->sid);
    thrum_tone_str(&b, "chunkType", "text_delta");
    thrum_tone_strn(&b, "delta", text, len);
    thrum_tone_end(&b);
    thrum_client_send(&g_client, &b);
    thrum_buf_free(&b);
}

static void send_finish(const turn *t, const char *reason, int exit_code, int tokens_in,
                        int tokens_out) {
    thrum_buf b;
    char usage[128];
    thrum_buf_init(&b);
    snprintf(usage, sizeof usage, "{\"inputTokens\":%d,\"outputTokens\":%d}", tokens_in,
             tokens_out);
    thrum_tone_begin(&b, CHI_FINISH, t->rid, t->sid);
    thrum_tone_str(&b, "finishReason", reason);
    thrum_tone_int(&b, "exitCode", exit_code);
    thrum_tone_raw(&b, "usage", usage);
    thrum_tone_end(&b);
    thrum_client_send(&g_client, &b);
    thrum_buf_free(&b);
}

static void send_error(const turn *t, const char *code, const char *message) {
    thrum_buf b;
    thrum_buf_init(&b);
    thrum_tone_begin(&b, CHI_ERROR, t->rid, t->sid);
    thrum_tone_str(&b, "code", code);
    thrum_tone_str(&b, "message", message);
    thrum_tone_end(&b);
    thrum_client_send(&g_client, &b);
    thrum_buf_free(&b);
}

static void on_piece(void *user, const char *text, size_t len) {
    send_chunk((const turn *)user, text, len);
}

static ds4_bridge *engine_open(void) {
    if (g_bridge) return g_bridge;
    g_bridge = ds4_bridge_new(g_ctx);
    return g_bridge;
}

static void *turn_worker(void *arg) {
    turn *t = (turn *)arg;
    char err[512];
    int rc;

    err[0] = '\0';
    if (!engine_open()) {
        send_error(t, "engine_unavailable", err);
        goto done;
    }
    rc = ds4_bridge_generate(g_bridge, t->model_path, t->system, t->prompt, g_temperature,
                             g_max_tokens, on_piece, t, err, sizeof err);
    if (rc != 0) {
        send_error(t, "generation_failed", err[0] ? err : "generation failed");
        send_finish(t, "error", 1, ds4_bridge_tokens_in(g_bridge),
                    ds4_bridge_tokens_out(g_bridge));
        goto done;
    }
    send_finish(t, "stop", 0, ds4_bridge_tokens_in(g_bridge), ds4_bridge_tokens_out(g_bridge));

done:
    free(t->prompt);
    free(t->system);
    free(t);
    return NULL;
}

static char *prompt_text(const thrum_json *tone) {
    const char *text = thrum_json_str(thrum_json_get(tone, "text"), NULL);
    const thrum_json *content;
    thrum_buf b;
    size_t i;

    if (text) {
        char *copy = (char *)malloc(strlen(text) + 1);
        if (copy) strcpy(copy, text);
        return copy;
    }
    content = thrum_json_get(tone, "content");
    if (!content || content->type != THRUM_JSON_ARRAY) return NULL;

    thrum_buf_init(&b);
    for (i = 0; i < content->item_count; i++) {
        const thrum_json *petal = content->items[i];
        const char *kind = thrum_json_str(thrum_json_get(petal, "type"), "text");
        const char *piece = thrum_json_str(thrum_json_get(petal, "text"), NULL);
        if (piece && strcmp(kind, "text") == 0) {
            if (b.len > 0) thrum_buf_append_str(&b, "\n");
            thrum_buf_append_str(&b, piece);
        }
    }
    if (b.len == 0) {
        thrum_buf_free(&b);
        return NULL;
    }
    return b.data; 
}

static void handle_prompt(const thrum_json *tone) {
    turn *t = (turn *)calloc(1, sizeof *t);
    pthread_t thread;
    const char *sid = thrum_json_str(thrum_json_get(tone, "sid"), "");
    const char *rid = thrum_json_str(thrum_json_get(tone, "rid"), "");
    const char *system = thrum_json_str(thrum_json_get(tone, "systemPrompt"), NULL);

    snprintf(t->sid, sizeof t->sid, "%s", sid);
    snprintf(t->rid, sizeof t->rid, "%s", rid);
    {
        const char *wanted = thrum_json_str(thrum_json_get(tone, "modelId"), "");
        const char *path = ds4_models_path(&g_models, wanted);
        if (path) snprintf(t->model_path, sizeof t->model_path, "%s", path);
    }
    t->prompt = prompt_text(tone);
    if (system) {
        t->system = (char *)malloc(strlen(system) + 1);
        if (t->system) strcpy(t->system, system);
    }
    if (!t->model_path[0]) {
        send_error(t, "no_such_model", "no DwarfStar model matches that model id");
        free(t->system);
        free(t);
        return;
    }
    if (!t->prompt) {
        send_error(t, "empty_prompt", "the prompt carried no text");
        free(t->system);
        free(t);
        return;
    }
    if (pthread_create(&thread, NULL, turn_worker, t) != 0) {
        send_error(t, "spawn_failed", "could not start the turn");
        free(t->prompt);
        free(t->system);
        free(t);
        return;
    }
    pthread_detach(thread);
}

static void on_tone(const thrum_json *tone, void *user) {
    const char *chi = thrum_json_str(thrum_json_get(tone, "chi"), "");
    (void)user;
    if (strcmp(chi, CHI_PROMPT) == 0) {
        handle_prompt(tone);
    } else if (strcmp(chi, CHI_CANCEL) == 0) {
        ds4_bridge_cancel(g_bridge);
    }
}

int main(void) {
    char metal_dir[2048];
    const char *chis =
        "[\"hello\",\"prompt\",\"cancel\",\"curate\",\"tool-result\",\"chunk\",\"finish\","
        "\"error\",\"tool-call\"]";
    const char *propensity =
        "{\"statefulness\":\"stateful_session\",\"wire\":\"dwarfstar/core\"}";
    const char *env;

    setvbuf(stdout, NULL, _IONBF, 0);

    if ((env = getenv("DWARFSTAR_DIR")) && env[0]) {
        snprintf(g_dir, sizeof g_dir, "%s", env);
    } else {
        snprintf(g_dir, sizeof g_dir, "%s/ds4", getenv("HOME") ? getenv("HOME") : ".");
    }
    if ((env = getenv("DWARFSTAR_CTX")) && env[0]) g_ctx = atoi(env);
    if ((env = getenv("DWARFSTAR_MAX_TOKENS")) && env[0]) g_max_tokens = atoi(env);
    if ((env = getenv("DWARFSTAR_TEMPERATURE")) && env[0]) g_temperature = (float)atof(env);

    if ((env = getenv("DWARFSTAR_METAL_DIR")) && env[0]) {
        snprintf(metal_dir, sizeof metal_dir, "%s", env);
    } else {
        char dir[2048];
        if (executable_dir(dir, sizeof dir) != 0) {
            fprintf(stderr, "dwarfstar: cannot find the metal kernels\n");
            return 1;
        }
        snprintf(metal_dir, sizeof metal_dir, "%s/metal", dir);
    }
    ds4_models_scan(&g_models, g_dir);

    if (dwarfstar_shaders_point_at(metal_dir) != 0) {
        fprintf(stderr, "dwarfstar: metal kernels not found under %s\n", metal_dir);
        return 1;
    }

    thrum_client_init(&g_client, NULL);
    if (thrum_client_connect(&g_client) != 0) return 1;

    if (!thrum_identity_load_or_mint(HIVE_NAME, "wbee", &g_identity)) {
        fprintf(stderr, "dwarfstar: could not load the bee identity\n");
        return 1;
    }
    {
        thrum_buf hello;
        char pubkey_hex[THRUM_PUBKEY_HEX_LEN];
        char sig_hex[THRUM_SIG_HEX_LEN];
        thrum_buf_init(&hello);
        thrum_tone_begin(&hello, CHI_HELLO, "hello-1", NULL);
        thrum_tone_raw(&hello, "bee", "[\"worker\"]");
        thrum_tone_str(&hello, "hive", HIVE_NAME);
        thrum_tone_str(&hello, "version", HIVE_VERSION);
        thrum_tone_str(&hello, "protoVersion", THRUM_VERSION);
        thrum_tone_str(&hello, "hid", g_identity.hid);
        thrum_tone_str(&hello, "from", g_identity.hid);
        if (thrum_identity_sign_hello(&g_identity, thrum_now_ms(), pubkey_hex, sig_hex)) {
            thrum_tone_str(&hello, "pubkey", pubkey_hex);
            thrum_tone_int(&hello, "signedAt", thrum_now_ms());
            thrum_tone_str(&hello, "signature", sig_hex);
        }
        thrum_tone_raw(&hello, "propensity", propensity);
        thrum_tone_raw(&hello, "chis", chis);
        {
            thrum_buf models;
            thrum_buf_init(&models);
            ds4_models_json(&g_models, &models);
            thrum_tone_raw(&hello, "models", models.data);
            thrum_buf_free(&models);
        }
        thrum_tone_str(&hello, "source", HIVE_SOURCE);
        thrum_tone_end(&hello);
        if (thrum_client_send(&g_client, &hello) != 0) return 1;
        thrum_buf_free(&hello);
    }

    
    {
        thrum_json *tone = NULL;
        int got = thrum_client_recv(&g_client, &tone);
        if (got != 1) {
            fprintf(stderr, "dwarfstar: no breath from humd\n");
            return 1;
        }
        {
            const char *chi = thrum_json_str(thrum_json_get(tone, "chi"), "");
            if (strcmp(chi, CHI_BREATH) != 0) {
                fprintf(stderr, "dwarfstar: hello refused: %s\n",
                        thrum_json_str(thrum_json_get(tone, "message"), "unknown"));
                thrum_json_free(tone);
                return 1;
            }
        }
        thrum_json_free(tone);
    }
    fprintf(stderr, "dwarfstar: nested as %s, models=%d dir=%s ctx=%d\n", g_identity.hid,
            g_models.count, g_dir, g_ctx);

    thrum_client_on_any(&g_client, on_tone, NULL);
    thrum_client_run(&g_client);

    thrum_client_free(&g_client);
    ds4_bridge_free(g_bridge);
    return 0;
}
