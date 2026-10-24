/* dwarfstar stub: the bridge without the engine, for make check.
 *
 * It lets the bee's thrum side run with no model and no Metal: entering,
 * the hello, the queue, chunks, finish and cancel. A turn answers with its
 * last message, a word per piece, then "é" split across two pieces, so
 * the bee's holding of a character a token ends inside of is exercised
 * too. The shader overrides are not needed and do nothing.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "ds4_bridge.h"
#include "shaders.h"

#define STUB_PIECE_MS 50

struct ds4_bridge {
    atomic_int cancelled;
};

ds4_bridge *ds4_bridge_new(int ctx_size) {
    ds4_bridge *bridge = (ds4_bridge *)calloc(1, sizeof *bridge);
    (void)ctx_size;
    if (bridge) atomic_init(&bridge->cancelled, 0);
    return bridge;
}

void ds4_bridge_free(ds4_bridge *bridge) {
    free(bridge);
}

void ds4_bridge_arm(ds4_bridge *bridge) {
    atomic_store(&bridge->cancelled, 0);
}

void ds4_bridge_cancel(ds4_bridge *bridge) {
    if (bridge) atomic_store(&bridge->cancelled, 1);
}

int ds4_bridge_generate(ds4_bridge *bridge, const char *model_path, const char *system,
                        const ds4_message *messages, size_t count, float temperature,
                        int max_tokens, ds4_piece_fn on_piece, void *user, ds4_turn *turn,
                        char *err, size_t errlen) {
    const char *text;
    const char *at;
    (void)model_path;
    (void)system;
    (void)temperature;

    turn->tokens_in = 0;
    turn->tokens_out = 0;
    turn->finish = "stop";
    err[0] = '\0';
    if (count == 0) {
        snprintf(err, errlen, "the prompt carried no text");
        return -1;
    }
    text = messages[count - 1].text;
    turn->tokens_in = (int)strlen(text);
    for (at = text; *at;) {
        size_t n = strcspn(at, " ");
        if (atomic_load(&bridge->cancelled)) {
            turn->finish = "cancelled";
            return 0;
        }
        if (turn->tokens_out >= max_tokens) {
            turn->finish = "length";
            return 0;
        }
        on_piece(user, at, n + (at[n] == ' ' ? 1 : 0));
        turn->tokens_out++;
        at += n + (at[n] == ' ' ? 1 : 0);
        usleep(STUB_PIECE_MS * 1000);
    }
    on_piece(user, " \xC3", 2);
    on_piece(user, "\xA9", 1);
    turn->tokens_out += 2;
    return 0;
}

int dwarfstar_shaders_point_at(const char *dir) {
    (void)dir;
    return 0;
}
