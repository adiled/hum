#ifndef DWARFSTAR_BRIDGE_H
#define DWARFSTAR_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>

#include "chat.h"

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
dialect ds4_bridge_dialect(const ds4_bridge *bridge);

void ds4_bridge_arm(ds4_bridge *bridge);
void ds4_bridge_cancel(ds4_bridge *bridge);
bool ds4_bridge_cancelled(ds4_bridge *bridge);

int ds4_bridge_generate(ds4_bridge *bridge, const char *system, const ds4_message *messages,
                        size_t count, float temperature, int max_tokens, ds4_piece_fn on_piece,
                        void *user, ds4_wilt *wilt, char *err, size_t errlen);
int ds4_bridge_generate_from_prompt(ds4_bridge *bridge, const char *prompt, float temperature,
                                    int max_tokens, ds4_piece_fn on_piece, void *user,
                                    ds4_wilt *wilt, char *err, size_t errlen);

#endif
