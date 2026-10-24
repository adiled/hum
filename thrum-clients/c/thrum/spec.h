/* thrum spec: what a tone may carry, judged the way humd judges it.
 *
 * Every chi's body is the view of the same name in protocol.h, and every
 * tone may carry the envelope. A key that is neither is undeclared: no
 * humd may rely on it and no bee should send it, and a bee's private data
 * belongs under ext. thrum_tone_undeclared mirrors thrum_core::undeclared:
 * it names the stray keys of a tone whose chi it knows and judges nothing
 * else.
 */

#ifndef THRUM_SPEC_H
#define THRUM_SPEC_H

#include <stdbool.h>
#include <stddef.h>

#include <cjson/cJSON.h>

#include "protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

const thrum_view *thrum_view_of(const char *chi);
bool thrum_key_is_declared(const char *chi, const char *key);
size_t thrum_tone_undeclared(const cJSON *tone, const char **stray, size_t cap);

#ifdef __cplusplus
}
#endif

#endif
