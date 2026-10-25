/* dwarfstar stub: the bridge without the engine, for make check.
 *
 * It lets the bee's thrum side and its GPU handoff run with no model and
 * no Metal: entering, the hello, the queue, the shared GPU lock, unfurls,
 * wilts and cancels. Loading takes a moment, as a real model's does. A
 * bloom answers with its last message, a word per piece, then "é" split
 * across two pieces, so the bee's holding of a character a token ends
 * inside of is exercised too. The shader overrides do nothing.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "ds4_bridge.h"
#include "shaders.h"

#define STUB_LOAD_MS 300
#define STUB_PIECE_MS 50

struct ds4_bridge {
    char path[1024];
    atomic_int cancelled;
};

static void sleep_ms(int ms) {
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

ds4_bridge *ds4_bridge_new(const ds4_bridge_options *options) {
    ds4_bridge *bridge = (ds4_bridge *)calloc(1, sizeof *bridge);
    (void)options;
    if (bridge) atomic_init(&bridge->cancelled, 0);
    return bridge;
}

void ds4_bridge_free(ds4_bridge *bridge) {
    free(bridge);
}

bool ds4_bridge_loaded(const ds4_bridge *bridge) {
    return bridge->path[0] != '\0';
}

bool ds4_bridge_ready(const ds4_bridge *bridge, const char *model_path) {
    return bridge->path[0] && strcmp(bridge->path, model_path) == 0;
}

int ds4_bridge_load(ds4_bridge *bridge, const char *model_path, char *err, size_t errlen) {
    if (strlen(model_path) >= sizeof bridge->path) {
        snprintf(err, errlen, "model path is too long");
        return -1;
    }
    sleep_ms(STUB_LOAD_MS);
    snprintf(bridge->path, sizeof bridge->path, "%s", model_path);
    return 0;
}

void ds4_bridge_unload(ds4_bridge *bridge) {
    bridge->path[0] = '\0';
}

void ds4_bridge_arm(ds4_bridge *bridge) {
    atomic_store(&bridge->cancelled, 0);
}

void ds4_bridge_cancel(ds4_bridge *bridge) {
    if (bridge) atomic_store(&bridge->cancelled, 1);
}

bool ds4_bridge_cancelled(ds4_bridge *bridge) {
    return atomic_load(&bridge->cancelled) != 0;
}

int ds4_bridge_generate(ds4_bridge *bridge, const char *system, const ds4_message *messages,
                        size_t count, float temperature, int max_tokens, ds4_piece_fn on_piece,
                        void *user, ds4_wilt *wilt, char *err, size_t errlen) {
    const char *text;
    const char *at;
    (void)system;
    (void)temperature;

    wilt->tokens_in = 0;
    wilt->tokens_out = 0;
    wilt->reason = "stop";
    err[0] = '\0';
    if (!bridge->path[0]) {
        snprintf(err, errlen, "no model is loaded");
        return -1;
    }
    if (count == 0) {
        snprintf(err, errlen, "the prompt carried no text");
        return -1;
    }
    text = messages[count - 1].text;
    wilt->tokens_in = (int)strlen(text);
    for (at = text; *at;) {
        size_t n = strcspn(at, " ");
        size_t step = n + (at[n] == ' ' ? 1 : 0);
        if (atomic_load(&bridge->cancelled)) {
            wilt->reason = "cancelled";
            return 0;
        }
        if (wilt->tokens_out >= max_tokens) {
            wilt->reason = "length";
            return 0;
        }
        on_piece(user, at, step);
        wilt->tokens_out++;
        at += step;
        sleep_ms(STUB_PIECE_MS);
    }
    on_piece(user, " \xC3", 2);
    on_piece(user, "\xA9", 1);
    wilt->tokens_out += 2;
    return 0;
}

int dwarfstar_shaders_point_at(const char *dir) {
    (void)dir;
    return 0;
}
