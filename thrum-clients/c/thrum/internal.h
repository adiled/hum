/* thrum internal: what the client's own .c files share and a bee never
 * includes. It comes first in every .c, so the POSIX feature macros are
 * set before any system header is read, whatever flags the including
 * build passes.
 */

#ifndef THRUM_INTERNAL_H
#define THRUM_INTERNAL_H

#if !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif
#if defined(__APPLE__) && !defined(_DARWIN_C_SOURCE)
#define _DARWIN_C_SOURCE
#endif

#include <stddef.h>

#include <cjson/cJSON.h>

#if defined(__GNUC__) || defined(__clang__)
#define THRUM_PRINTF(fmt, args) __attribute__((format(printf, fmt, args)))
#else
#define THRUM_PRINTF(fmt, args)
#endif

int thrum__fail(const char *fmt, ...) THRUM_PRINTF(1, 2);

char *thrum__slurp(const char *path, size_t max, size_t *len);

int thrum__set_string(cJSON *object, const char *key, const char *value);

int thrum__set_number(cJSON *object, const char *key, double value);

#endif
