/* dwarfstar bridge — the DwarfStar engine, embedded. See ds4_bridge.h. */

#include "ds4_bridge.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include "ds4.h"

#define DS4_MAX_ENGINES 8

struct ds4_bridge {
    char path[DS4_MAX_ENGINES][1024];
    ds4_engine *engine[DS4_MAX_ENGINES];
    int engine_count;
    int ctx_size;
    volatile int cancelled;
    int tokens_in;
    int tokens_out;
};

static bool session_cancelled(void *user) {
    ds4_bridge *bridge = (ds4_bridge *)user;
    return bridge->cancelled != 0;
}

ds4_bridge *ds4_bridge_new(int ctx_size) {
    ds4_bridge *bridge = (ds4_bridge *)calloc(1, sizeof *bridge);
    if (!bridge) {
        fprintf(stderr, "dwarfstar: out of memory\n");
        exit(70);
    }
    bridge->ctx_size = ctx_size;
    return bridge;
}

void ds4_bridge_free(ds4_bridge *bridge) {
    int i;
    if (!bridge) return;
    for (i = 0; i < bridge->engine_count; i++) {
        if (bridge->engine[i]) ds4_engine_close(bridge->engine[i]);
    }
    free(bridge);
}

void ds4_bridge_cancel(ds4_bridge *bridge) {
    if (bridge) bridge->cancelled = 1;
}

int ds4_bridge_tokens_in(const ds4_bridge *bridge) {
    return bridge ? bridge->tokens_in : 0;
}

int ds4_bridge_tokens_out(const ds4_bridge *bridge) {
    return bridge ? bridge->tokens_out : 0;
}

static ds4_engine *engine_for(ds4_bridge *bridge, const char *model_path, char *err,
                              size_t errlen) {
    ds4_engine_options opt;
    struct stat st;
    int i;

    for (i = 0; i < bridge->engine_count; i++) {
        if (strcmp(bridge->path[i], model_path) == 0) return bridge->engine[i];
    }
    if (bridge->engine_count >= DS4_MAX_ENGINES) {
        snprintf(err, errlen, "too many models loaded");
        return NULL;
    }
    if (stat(model_path, &st) != 0) {
        snprintf(err, errlen, "model file not found: %s", model_path);
        return NULL;
    }
    if (!S_ISREG(st.st_mode)) {
        snprintf(err, errlen, "model path is not a file: %s", model_path);
        return NULL;
    }
    if (st.st_size < 32) {
        snprintf(err, errlen, "model file is too small to be GGUF: %s", model_path);
        return NULL;
    }
    {
        FILE *probe = fopen(model_path, "rb");
        char magic[4] = {0};
        if (!probe) {
            snprintf(err, errlen, "model file cannot be read: %s", model_path);
            return NULL;
        }
        size_t got = fread(magic, 1, 4, probe);
        fclose(probe);
        if (got != 4 || memcmp(magic, "GGUF", 4) != 0) {
            snprintf(err, errlen, "not a GGUF file: %s", model_path);
            return NULL;
        }
    }

    memset(&opt, 0, sizeof opt);
    opt.model_path = model_path;
    opt.context_size = bridge->ctx_size;
    opt.placement_ctx_hint = bridge->ctx_size;
#if defined(__APPLE__)
    opt.backend = DS4_BACKEND_METAL;
#else
    opt.backend = DS4_BACKEND_CUDA;
#endif

    if (ds4_engine_open(&bridge->engine[bridge->engine_count], &opt) != 0 ||
        !bridge->engine[bridge->engine_count]) {
        snprintf(err, errlen, "ds4: could not open the engine on %s", model_path);
        return NULL;
    }
    snprintf(bridge->path[bridge->engine_count], sizeof bridge->path[0], "%s", model_path);
    bridge->engine_count++;
    return bridge->engine[bridge->engine_count - 1];
}

int ds4_bridge_generate(ds4_bridge *bridge, const char *model_path, const char *system,
                        const char *prompt, float temperature, int max_tokens,
                        ds4_piece_fn on_piece, void *user, char *err, size_t errlen) {
    ds4_engine *engine;
    ds4_session *session = NULL;
    ds4_tokens tokens;
    uint64_t rng = 0;
    int rc = -1;
    int produced = 0;

    if (!bridge) {
        snprintf(err, errlen, "no engine");
        return -1;
    }
    engine = engine_for(bridge, model_path, err, errlen);
    if (!engine) return -1;

    bridge->cancelled = 0;
    bridge->tokens_in = 0;
    bridge->tokens_out = 0;

    memset(&tokens, 0, sizeof tokens);
    ds4_encode_chat_prompt(engine, system, prompt, DS4_THINK_NONE, &tokens);
    bridge->tokens_in = tokens.len;

    if (ds4_session_create(&session, engine, bridge->ctx_size) != 0) {
        snprintf(err, errlen, "ds4: could not create a session");
        ds4_tokens_free(&tokens);
        return -1;
    }
    ds4_session_set_cancel(session, session_cancelled, bridge);

    if (ds4_session_sync(session, &tokens, err, errlen) != 0) {
        ds4_session_free(session);
        ds4_tokens_free(&tokens);
        return -1;
    }

    while (produced < max_tokens) {
        int token;
        size_t piece_len = 0;
        char *piece;

        if (bridge->cancelled) break;

        token = ds4_session_sample(session, temperature, 0, 0.95f, 0.05f, &rng);
        if (token < 0) {
            snprintf(err, errlen, "ds4: sampling failed");
            goto done;
        }
        if (ds4_token_is_stop(engine, token)) break;

        if (ds4_session_eval(session, token, err, errlen) != 0) goto done;

        piece = ds4_token_text(engine, token, &piece_len);
        if (piece) {
            if (on_piece && piece_len > 0) on_piece(user, piece, piece_len);
            free(piece);
        }
        produced++;
        bridge->tokens_out = produced;
    }
    rc = 0;

done:
    ds4_session_set_cancel(session, NULL, NULL);
    ds4_session_free(session);
    ds4_tokens_free(&tokens);
    return rc;
}
