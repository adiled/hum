/* thrum helpers: the primitives every thrum client carries, in C.
 *
 * The same algorithms as thrum-core/src/{prims,wane}.rs and the helpers
 * the TS, Python and Go clients are generated with:
 *
 *   thrum_sigil(sid, nest)     12 hex chars of sha256("nest:sid")
 *   thrum_rid(out)             52-char Crockford base32 HumId, ms-prefixed
 *   thrum_humid_encode(b, out) the encoder under it, bit for bit
 *   thrum_dusk_in / is_dusk    expiry stamps on a tone
 *   thrum_wane_*               one Lamport counter per sigil, thread-safe
 *   thrum_default_socket_path  HUM_THRUM_SOCK > HUM_SOCKET > runtime.json
 *                              > $XDG_STATE_HOME/hum/thrum.sock
 *
 * Hashing and randomness are libsodium's, JSON is cJSON's. Nothing here
 * is a hand-rolled primitive. Functions that can fail return -1 and leave
 * the reason in thrum_error(), which is per thread.
 */

#ifndef THRUM_HELPERS_H
#define THRUM_HELPERS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <cjson/cJSON.h>

#ifdef __cplusplus
extern "C" {
#endif

#define THRUM_SIGIL_LEN 13
#define THRUM_RID_LEN 53

const char *thrum_error(void);

int thrum_init(void);

int64_t thrum_now_ms(void);
int64_t thrum_dusk_in(int64_t ms);
bool thrum_is_dusk(const cJSON *tone);

void thrum_sigil(const char *sid, const char *nest, char out[THRUM_SIGIL_LEN]);

void thrum_humid_encode(const uint8_t bytes[32], char out[THRUM_RID_LEN]);
void thrum_rid(char out[THRUM_RID_LEN]);
bool thrum_is_valid_rid(const char *value);

int thrum_state_dir(char *out, size_t cap);
int thrum_default_socket_path(char *out, size_t cap);

typedef struct thrum_wane thrum_wane;

thrum_wane *thrum_wane_new(void);
void thrum_wane_free(thrum_wane *wane);
int64_t thrum_wane_get(thrum_wane *wane, const char *sigil);
int64_t thrum_wane_tick(thrum_wane *wane, const char *sigil);
int thrum_wane_set(thrum_wane *wane, const char *sigil, int64_t value);
bool thrum_wane_behind(thrum_wane *wane, const char *sigil, int64_t remote);

#ifdef __cplusplus
}
#endif

#endif
