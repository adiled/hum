/* dwarfstar bridge — the DwarfStar engine, embedded.
 *
 * The engine is a C library (ds4.h). This file is the only place that
 * talks to it: the bee above speaks thrum, the bridge speaks ds4.
 *
 * The generation loop copies the order ds4-server itself uses:
 *   sync(prompt) -> sample -> stop-check -> eval -> detokenize -> emit
 */

#ifndef DWARFSTAR_BRIDGE_H
#define DWARFSTAR_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>

typedef struct ds4_bridge ds4_bridge;

typedef void (*ds4_piece_fn)(void *user, const char *text, size_t len);

ds4_bridge *ds4_bridge_new(int ctx_size);
void ds4_bridge_free(ds4_bridge *bridge);

int ds4_bridge_generate(ds4_bridge *bridge, const char *model_path, const char *system,
                        const char *prompt, float temperature, int max_tokens,
                        ds4_piece_fn on_piece, void *user, char *err, size_t errlen);

void ds4_bridge_cancel(ds4_bridge *bridge);

int ds4_bridge_tokens_in(const ds4_bridge *bridge);
int ds4_bridge_tokens_out(const ds4_bridge *bridge);

#endif /* DWARFSTAR_BRIDGE_H */
