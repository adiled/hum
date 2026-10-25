/* dwarfstar bridge: the only part of the bee that talks to the DwarfStar
 * engine (ds4.h).
 *
 * The bee loads a model with ds4_bridge_load once it holds the GPU, and
 * unloads it with ds4_bridge_unload before letting the GPU go. Loading
 * first checks ds4's own lock (DS4_LOCK_FILE), because ds4 exits when
 * another process holds it; a held lock is an error here instead. The
 * loaded engine keeps one session, and every bloom syncs that session to
 * the bloom's whole prompt, so ds4 evaluates only the part the KV cache
 * does not already hold. The generation loop is ds4-server's: sync,
 * sample, stop check, eval, detokenize, emit.
 *
 * Load, unload and generate run on one thread. ds4_bridge_arm,
 * ds4_bridge_cancel and ds4_bridge_cancelled may be called from any
 * thread: arm clears the cancel before a bloom starts, cancel asks the bloom
 * in flight to stop, including one still waiting for the GPU.
 */

#ifndef DWARFSTAR_BRIDGE_H
#define DWARFSTAR_BRIDGE_H

#include <stdbool.h>
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
    const char *reason;
} ds4_wilt;

typedef struct {
    int ctx_size;
    int threads;
    bool ssd_streaming;
} ds4_bridge_options;

ds4_bridge *ds4_bridge_new(const ds4_bridge_options *options);
void ds4_bridge_free(ds4_bridge *bridge);

bool ds4_bridge_loaded(const ds4_bridge *bridge);
bool ds4_bridge_ready(const ds4_bridge *bridge, const char *model_path);
int ds4_bridge_load(ds4_bridge *bridge, const char *model_path, char *err, size_t errlen);
void ds4_bridge_unload(ds4_bridge *bridge);

void ds4_bridge_arm(ds4_bridge *bridge);
void ds4_bridge_cancel(ds4_bridge *bridge);
bool ds4_bridge_cancelled(ds4_bridge *bridge);

int ds4_bridge_generate(ds4_bridge *bridge, const char *system, const ds4_message *messages,
                        size_t count, float temperature, int max_tokens, ds4_piece_fn on_piece,
                        void *user, ds4_wilt *wilt, char *err, size_t errlen);

#endif
