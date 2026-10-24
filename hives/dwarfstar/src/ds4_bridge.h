/* dwarfstar bridge: the only part of the bee that talks to the DwarfStar
 * engine (ds4.h).
 *
 * One engine is resident at a time, because a model is tens of gigabytes:
 * asking for another model closes the one that is loaded. The engine keeps
 * one session for as long as it is loaded, and every turn syncs that
 * session to the turn's whole prompt, so ds4 evaluates only the part the
 * KV cache does not already hold. The generation loop is ds4-server's:
 * sync, sample, stop check, eval, detokenize, emit.
 *
 * Turns run one at a time, on one thread. ds4_bridge_arm and
 * ds4_bridge_cancel may be called from any thread: arm clears the cancel
 * before a turn starts, cancel asks the turn in flight to stop.
 */

#ifndef DWARFSTAR_BRIDGE_H
#define DWARFSTAR_BRIDGE_H

#include <stddef.h>

typedef struct ds4_bridge ds4_bridge;

typedef void (*ds4_piece_fn)(void *user, const char *text, size_t len);

typedef struct {
    const char *role;
    const char *text;
} ds4_message;

typedef struct {
    int tokens_in;
    int tokens_out;
    const char *finish;
} ds4_turn;

ds4_bridge *ds4_bridge_new(int ctx_size);
void ds4_bridge_free(ds4_bridge *bridge);

void ds4_bridge_arm(ds4_bridge *bridge);
void ds4_bridge_cancel(ds4_bridge *bridge);

int ds4_bridge_generate(ds4_bridge *bridge, const char *model_path, const char *system,
                        const ds4_message *messages, size_t count, float temperature,
                        int max_tokens, ds4_piece_fn on_piece, void *user, ds4_turn *turn,
                        char *err, size_t errlen);

#endif
