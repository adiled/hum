/* dwarfstar bridge: one resident engine, one long-lived session, and the
 * generation loop. See ds4_bridge.h.
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include "ds4_bridge.h"

#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include <sodium.h>

#include "ds4.h"

#define GGUF_MAGIC "GGUF"
#define GGUF_MIN_BYTES 32
#define SEED_IF_ZERO UINT64_C(0x9e3779b97f4a7c15)

struct ds4_bridge {
    char path[1024];
    ds4_engine *engine;
    ds4_session *session;
    int ctx_size;
    atomic_int cancelled;
};

static bool session_cancelled(void *user) {
    return atomic_load(&((ds4_bridge *)user)->cancelled) != 0;
}

ds4_bridge *ds4_bridge_new(int ctx_size) {
    ds4_bridge *bridge = (ds4_bridge *)calloc(1, sizeof *bridge);
    if (!bridge) return NULL;
    bridge->ctx_size = ctx_size;
    atomic_init(&bridge->cancelled, 0);
    if (sodium_init() < 0) {
        free(bridge);
        return NULL;
    }
    return bridge;
}

static void unload(ds4_bridge *bridge) {
    if (bridge->session) ds4_session_free(bridge->session);
    if (bridge->engine) ds4_engine_close(bridge->engine);
    bridge->session = NULL;
    bridge->engine = NULL;
    bridge->path[0] = '\0';
}

void ds4_bridge_free(ds4_bridge *bridge) {
    if (!bridge) return;
    unload(bridge);
    free(bridge);
}

void ds4_bridge_arm(ds4_bridge *bridge) {
    atomic_store(&bridge->cancelled, 0);
}

void ds4_bridge_cancel(ds4_bridge *bridge) {
    if (bridge) atomic_store(&bridge->cancelled, 1);
}

static int is_gguf(const char *path, char *err, size_t errlen) {
    struct stat st;
    char magic[4] = {0};
    size_t got;
    FILE *probe;

    if (stat(path, &st) != 0) {
        snprintf(err, errlen, "model file not found: %s", path);
        return -1;
    }
    if (!S_ISREG(st.st_mode) || st.st_size < GGUF_MIN_BYTES) {
        snprintf(err, errlen, "not a model file: %s", path);
        return -1;
    }
    probe = fopen(path, "rb");
    if (!probe) {
        snprintf(err, errlen, "model file cannot be read: %s", path);
        return -1;
    }
    got = fread(magic, 1, sizeof magic, probe);
    fclose(probe);
    if (got != sizeof magic || memcmp(magic, GGUF_MAGIC, sizeof magic) != 0) {
        snprintf(err, errlen, "not a GGUF file: %s", path);
        return -1;
    }
    return 0;
}

static int load(ds4_bridge *bridge, const char *model_path, char *err, size_t errlen) {
    ds4_engine_options opt;

    if (bridge->engine && strcmp(bridge->path, model_path) == 0) return 0;
    if (strlen(model_path) >= sizeof bridge->path) {
        snprintf(err, errlen, "model path is too long: %s", model_path);
        return -1;
    }
    if (is_gguf(model_path, err, errlen) != 0) return -1;
    unload(bridge);

    memset(&opt, 0, sizeof opt);
    opt.model_path = model_path;
    opt.backend = DS4_BACKEND_METAL;
    opt.context_size = bridge->ctx_size;
    opt.placement_ctx_hint = bridge->ctx_size;
    if (ds4_engine_open(&bridge->engine, &opt) != 0 || !bridge->engine) {
        bridge->engine = NULL;
        snprintf(err, errlen, "ds4 could not open %s", model_path);
        return -1;
    }
    if (ds4_session_create(&bridge->session, bridge->engine, bridge->ctx_size) != 0 ||
        !bridge->session) {
        bridge->session = NULL;
        unload(bridge);
        snprintf(err, errlen, "ds4 could not create a session of %d tokens", bridge->ctx_size);
        return -1;
    }
    ds4_session_set_cancel(bridge->session, session_cancelled, bridge);
    memcpy(bridge->path, model_path, strlen(model_path) + 1);
    return 0;
}

static void encode(ds4_engine *engine, const char *system, const ds4_message *messages,
                   size_t count, ds4_tokens *out) {
    size_t i;
    if (count == 1 && (!messages[0].role || strcmp(messages[0].role, "user") == 0)) {
        ds4_encode_chat_prompt(engine, system, messages[0].text, DS4_THINK_NONE, out);
        return;
    }
    ds4_chat_begin(engine, out);
    if (system && system[0]) ds4_chat_append_message(engine, out, "system", system);
    for (i = 0; i < count; i++) {
        ds4_chat_append_message(engine, out, messages[i].role ? messages[i].role : "user",
                                messages[i].text);
    }
    ds4_chat_append_assistant_prefix(engine, out, DS4_THINK_NONE);
}

static uint64_t seed(void) {
    uint64_t rng = 0;
    randombytes_buf(&rng, sizeof rng);
    return rng ? rng : SEED_IF_ZERO;
}

int ds4_bridge_generate(ds4_bridge *bridge, const char *model_path, const char *system,
                        const ds4_message *messages, size_t count, float temperature,
                        int max_tokens, ds4_piece_fn on_piece, void *user, ds4_turn *turn,
                        char *err, size_t errlen) {
    ds4_tokens tokens;
    uint64_t rng = seed();
    int rc;

    turn->tokens_in = 0;
    turn->tokens_out = 0;
    turn->finish = "stop";
    err[0] = '\0';
    if (count == 0) {
        snprintf(err, errlen, "the prompt carried no text");
        return -1;
    }
    if (load(bridge, model_path, err, errlen) != 0) return -1;
    if (temperature < 0) temperature = DS4_DEFAULT_TEMPERATURE;

    memset(&tokens, 0, sizeof tokens);
    encode(bridge->engine, system, messages, count, &tokens);
    turn->tokens_in = tokens.len;
    rc = ds4_session_sync(bridge->session, &tokens, err, errlen);
    ds4_tokens_free(&tokens);
    if (rc == DS4_SESSION_SYNC_INTERRUPTED) {
        turn->finish = "cancelled";
        return 0;
    }
    if (rc != 0) {
        if (!err[0]) snprintf(err, errlen, "ds4 could not read the prompt");
        return -1;
    }

    for (;;) {
        int token;
        size_t len = 0;
        char *piece;

        if (atomic_load(&bridge->cancelled)) {
            turn->finish = "cancelled";
            break;
        }
        if (turn->tokens_out >= max_tokens ||
            ds4_session_pos(bridge->session) >= ds4_session_ctx(bridge->session)) {
            turn->finish = "length";
            break;
        }
        token = ds4_session_sample(bridge->session, temperature, 0, DS4_DEFAULT_TOP_P,
                                   DS4_DEFAULT_MIN_P, &rng);
        if (token < 0) {
            snprintf(err, errlen, "ds4 could not sample a token");
            return -1;
        }
        if (ds4_token_is_stop_for_think_mode(bridge->engine, token, DS4_THINK_NONE)) break;
        if (ds4_session_eval(bridge->session, token, err, errlen) != 0) {
            if (!err[0]) snprintf(err, errlen, "ds4 could not evaluate a token");
            return -1;
        }
        piece = ds4_token_text(bridge->engine, token, &len);
        if (piece) {
            if (len > 0 && on_piece) on_piece(user, piece, len);
            free(piece);
        }
        turn->tokens_out++;
    }
    return 0;
}
