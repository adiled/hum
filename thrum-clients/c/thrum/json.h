/* thrum json — the smallest JSON that carries a tone.
 *
 * NDJSON is the wire: one JSON object per line. A C bee has no stdlib
 * JSON, so this is the piece the spec assumes you bring. It parses the
 * whole JSON grammar (strings with \u escapes, numbers, arrays, objects,
 * true/false/null) and offers a growable buffer for writing tones back.
 *
 * Strings are NUL-terminated and carry their byte length separately, so
 * an embedded NUL inside a string stays intact.
 */

#ifndef THRUM_JSON_H
#define THRUM_JSON_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    THRUM_JSON_NULL,
    THRUM_JSON_BOOL,
    THRUM_JSON_NUMBER,
    THRUM_JSON_STRING,
    THRUM_JSON_ARRAY,
    THRUM_JSON_OBJECT
} thrum_json_type;

typedef struct thrum_json thrum_json;

struct thrum_json {
    thrum_json_type type;
    bool boolean;
    double number;
    char *string;      
    size_t string_len;
    thrum_json **items; 
    size_t item_count;
    char **keys;       
    thrum_json **values;
    size_t pair_count;
};

thrum_json *thrum_json_parse(const char *text, size_t len);
void thrum_json_free(thrum_json *value);

thrum_json *thrum_json_get(const thrum_json *value, const char *key);
const char *thrum_json_key_at(const thrum_json *value, size_t index);
thrum_json *thrum_json_value_at(const thrum_json *value, size_t index);

const char *thrum_json_str(const thrum_json *value, const char *fallback);
double thrum_json_num(const thrum_json *value, double fallback);
bool thrum_json_bool(const thrum_json *value, bool fallback);

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} thrum_buf;

void thrum_buf_init(thrum_buf *buf);
void thrum_buf_free(thrum_buf *buf);
void thrum_buf_append(thrum_buf *buf, const char *bytes, size_t len);
void thrum_buf_append_str(thrum_buf *buf, const char *text);

void thrum_buf_append_json_string(thrum_buf *buf, const char *text);

void thrum_buf_append_json_string_n(thrum_buf *buf, const char *text, size_t len);

void thrum_buf_append_json_raw(thrum_buf *buf, const char *json);

#ifdef __cplusplus
}
#endif

#endif 
