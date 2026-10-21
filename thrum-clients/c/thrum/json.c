/* thrum json — the tone parser and writer.
 *
 * Parsing is jsmn's job (vendored, MIT — see jsmn.h). This file turns
 * jsmn's token stream into a small tree, unescapes strings on the way,
 * and owns the writer.
 *
 * The writer is hand-written on purpose: the shapes it emits are fixed
 * (a tone is an envelope plus a handful of known fields), so a dozen
 * lines of escaping is safer than pulling in a second library.
 */

#include "json.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "jsmn.h"

static void *xmalloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) {
        fprintf(stderr, "thrum: out of memory\n");
        exit(70);
    }
    return p;
}

static void *xrealloc(void *p, size_t n) {
    void *q = realloc(p, n ? n : 1);
    if (!q) {
        fprintf(stderr, "thrum: out of memory\n");
        exit(70);
    }
    return q;
}

static thrum_json *node_new(thrum_json_type type) {
    thrum_json *v = (thrum_json *)xmalloc(sizeof *v);
    memset(v, 0, sizeof *v);
    v->type = type;
    return v;
}

void thrum_json_free(thrum_json *value) {
    size_t i;
    if (!value) return;
    free(value->string);
    for (i = 0; i < value->item_count; i++) thrum_json_free(value->items[i]);
    free(value->items);
    for (i = 0; i < value->pair_count; i++) {
        free(value->keys[i]);
        thrum_json_free(value->values[i]);
    }
    free(value->keys);
    free(value->values);
    free(value);
}

static char *unescape(const char *src, size_t len, size_t *out_len) {
    char *out = (char *)xmalloc(len + 1);
    size_t i = 0, o = 0;
    while (i < len) {
        char c = src[i];
        if (c != '\\' || i + 1 >= len) {
            out[o++] = c;
            i++;
            continue;
        }
        i++;
        switch (src[i]) {
            case 'n': out[o++] = '\n'; i++; break;
            case 't': out[o++] = '\t'; i++; break;
            case 'r': out[o++] = '\r'; i++; break;
            case 'b': out[o++] = '\b'; i++; break;
            case 'f': out[o++] = '\f'; i++; break;
            case '/': out[o++] = '/'; i++; break;
            case '"': out[o++] = '"'; i++; break;
            case '\\': out[o++] = '\\'; i++; break;
            case 'u': {
                unsigned cp = 0;
                int k;
                if (i + 4 >= len) {
                    out[o++] = 'u';
                    i++;
                    break;
                }
                for (k = 1; k <= 4; k++) {
                    char h = src[i + (size_t)k];
                    cp <<= 4;
                    if (h >= '0' && h <= '9') cp |= (unsigned)(h - '0');
                    else if (h >= 'a' && h <= 'f') cp |= (unsigned)(h - 'a' + 10);
                    else if (h >= 'A' && h <= 'F') cp |= (unsigned)(h - 'A' + 10);
                    else cp = 0xfffd;
                }
                i += 5;
                if (cp < 0x80) {
                    out[o++] = (char)cp;
                } else if (cp < 0x800) {
                    out[o++] = (char)(0xC0 | (cp >> 6));
                    out[o++] = (char)(0x80 | (cp & 0x3F));
                } else {
                    out[o++] = (char)(0xE0 | (cp >> 12));
                    out[o++] = (char)(0x80 | ((cp >> 6) & 0x3F));
                    out[o++] = (char)(0x80 | (cp & 0x3F));
                }
                break;
            }
            default: out[o++] = src[i]; i++; break;
        }
    }
    out[o] = '\0';
    if (out_len) *out_len = o;
    return out;
}

static thrum_json *build(const jsmntok_t *toks, int *idx, const char *text);

static thrum_json *build_object(const jsmntok_t *toks, int *idx, const char *text) {
    thrum_json *obj = node_new(THRUM_JSON_OBJECT);
    size_t cap = 8;
    
    int pairs_left = toks[*idx].size;
    obj->keys = (char **)xmalloc(cap * sizeof(char *));
    obj->values = (thrum_json **)xmalloc(cap * sizeof(thrum_json *));
    (*idx)++;
    while (pairs_left > 0) {
        const jsmntok_t *t = &toks[*idx];
        if (t->type != JSMN_STRING) break;
        {
            char *key = unescape(text + t->start, (size_t)(t->end - t->start), NULL);
            thrum_json *val;
            (*idx)++;
            val = build(toks, idx, text);
            if (!val) {
                free(key);
                thrum_json_free(obj);
                return NULL;
            }
            if (obj->pair_count == cap) {
                cap *= 2;
                obj->keys = (char **)xrealloc(obj->keys, cap * sizeof(char *));
                obj->values = (thrum_json **)xrealloc(obj->values, cap * sizeof(thrum_json *));
            }
            obj->keys[obj->pair_count] = key;
            obj->values[obj->pair_count] = val;
            obj->pair_count++;
            pairs_left--;
        }
    }
    return obj;
}

static thrum_json *build_array(const jsmntok_t *toks, int *idx, const char *text) {
    thrum_json *arr = node_new(THRUM_JSON_ARRAY);
    size_t cap = 8;
    int items_left = toks[*idx].size;
    arr->items = (thrum_json **)xmalloc(cap * sizeof(thrum_json *));
    (*idx)++;
    while (items_left > 0) {
        thrum_json *val = build(toks, idx, text);
        if (!val) {
            thrum_json_free(arr);
            return NULL;
        }
        if (arr->item_count == cap) {
            cap *= 2;
            arr->items = (thrum_json **)xrealloc(arr->items, cap * sizeof(thrum_json *));
        }
        arr->items[arr->item_count++] = val;
        items_left--;
    }
    return arr;
}

static thrum_json *build(const jsmntok_t *toks, int *idx, const char *text) {
    const jsmntok_t *t = &toks[*idx];
    switch (t->type) {
        case JSMN_OBJECT:
            return build_object(toks, idx, text);
        case JSMN_ARRAY:
            return build_array(toks, idx, text);
        case JSMN_STRING: {
            thrum_json *v = node_new(THRUM_JSON_STRING);
            v->string = unescape(text + t->start, (size_t)(t->end - t->start), &v->string_len);
            (*idx)++;
            return v;
        }
        case JSMN_PRIMITIVE: {
            size_t len = (size_t)(t->end - t->start);
            const char *p = text + t->start;
            thrum_json *v;
            if (len == 4 && memcmp(p, "true", 4) == 0) {
                v = node_new(THRUM_JSON_BOOL);
                v->boolean = true;
            } else if (len == 5 && memcmp(p, "false", 5) == 0) {
                v = node_new(THRUM_JSON_BOOL);
                v->boolean = false;
            } else if (len == 4 && memcmp(p, "null", 4) == 0) {
                v = node_new(THRUM_JSON_NULL);
            } else {
                v = node_new(THRUM_JSON_NUMBER);
                v->number = strtod(p, NULL);
            }
            (*idx)++;
            return v;
        }
        default:
            (*idx)++;
            return node_new(THRUM_JSON_NULL);
    }
}

thrum_json *thrum_json_parse(const char *text, size_t len) {
    jsmn_parser parser;
    jsmntok_t *tokens = NULL;
    size_t cap = 64;
    int count;
    int idx = 0;
    thrum_json *root;
    char *copy;

    copy = (char *)xmalloc(len + 1);
    memcpy(copy, text, len);
    copy[len] = '\0';

    jsmn_init(&parser);
    for (;;) {
        tokens = (jsmntok_t *)xrealloc(tokens, cap * sizeof(jsmntok_t));
        count = jsmn_parse(&parser, copy, len, tokens, (int)cap);
        if (count == JSMN_ERROR_NOMEM) {
            cap *= 2;
            if (cap > 1u << 20) {
                free(tokens);
                free(copy);
                return NULL;
            }
            continue;
        }
        break;
    }
    if (count < 1) {
        free(tokens);
        free(copy);
        return NULL;
    }

    root = build(tokens, &idx, copy);
    free(tokens);
    free(copy);
    return root;
}

thrum_json *thrum_json_get(const thrum_json *value, const char *key) {
    size_t i;
    if (!value || value->type != THRUM_JSON_OBJECT || !key) return NULL;
    for (i = 0; i < value->pair_count; i++) {
        if (strcmp(value->keys[i], key) == 0) return value->values[i];
    }
    return NULL;
}

const char *thrum_json_key_at(const thrum_json *value, size_t index) {
    if (!value || value->type != THRUM_JSON_OBJECT || index >= value->pair_count)
        return NULL;
    return value->keys[index];
}

thrum_json *thrum_json_value_at(const thrum_json *value, size_t index) {
    if (!value || value->type != THRUM_JSON_OBJECT || index >= value->pair_count)
        return NULL;
    return value->values[index];
}

const char *thrum_json_str(const thrum_json *value, const char *fallback) {
    if (!value || value->type != THRUM_JSON_STRING) return fallback;
    return value->string;
}

double thrum_json_num(const thrum_json *value, double fallback) {
    if (!value || value->type != THRUM_JSON_NUMBER) return fallback;
    return value->number;
}

bool thrum_json_bool(const thrum_json *value, bool fallback) {
    if (!value || value->type != THRUM_JSON_BOOL) return fallback;
    return value->boolean;
}

void thrum_buf_init(thrum_buf *buf) {
    buf->data = NULL;
    buf->len = 0;
    buf->cap = 0;
}

void thrum_buf_free(thrum_buf *buf) {
    free(buf->data);
    buf->data = NULL;
    buf->len = 0;
    buf->cap = 0;
}

static void buf_reserve(thrum_buf *buf, size_t extra) {
    size_t need = buf->len + extra + 1;
    if (need <= buf->cap) return;
    if (buf->cap == 0) buf->cap = 256;
    while (buf->cap < need) buf->cap *= 2;
    buf->data = (char *)xrealloc(buf->data, buf->cap);
}

void thrum_buf_append(thrum_buf *buf, const char *bytes, size_t len) {
    buf_reserve(buf, len);
    memcpy(buf->data + buf->len, bytes, len);
    buf->len += len;
    buf->data[buf->len] = '\0';
}

void thrum_buf_append_str(thrum_buf *buf, const char *text) {
    if (!text) return;
    thrum_buf_append(buf, text, strlen(text));
}

void thrum_buf_append_json_string(thrum_buf *buf, const char *text) {
    const unsigned char *p = (const unsigned char *)(text ? text : "");
    thrum_buf_append(buf, "\"", 1);
    for (; *p; p++) {
        switch (*p) {
            case '"': thrum_buf_append(buf, "\\\"", 2); break;
            case '\\': thrum_buf_append(buf, "\\\\", 2); break;
            case '\n': thrum_buf_append(buf, "\\n", 2); break;
            case '\r': thrum_buf_append(buf, "\\r", 2); break;
            case '\t': thrum_buf_append(buf, "\\t", 2); break;
            case '\b': thrum_buf_append(buf, "\\b", 2); break;
            case '\f': thrum_buf_append(buf, "\\f", 2); break;
            default:
                if (*p < 0x20) {
                    char esc[8];
                    snprintf(esc, sizeof esc, "\\u%04x", *p);
                    thrum_buf_append(buf, esc, 6);
                } else {
                    thrum_buf_append(buf, (const char *)p, 1);
                }
        }
    }
    thrum_buf_append(buf, "\"", 1);
}

void thrum_buf_append_json_string_n(thrum_buf *buf, const char *text, size_t len) {
    const unsigned char *p = (const unsigned char *)(text ? text : "");
    size_t i;
    thrum_buf_append(buf, "\"", 1);
    for (i = 0; i < len && p[i]; i++) {
        switch (p[i]) {
            case '"': thrum_buf_append(buf, "\\\"", 2); break;
            case '\\': thrum_buf_append(buf, "\\\\", 2); break;
            case '\n': thrum_buf_append(buf, "\\n", 2); break;
            case '\r': thrum_buf_append(buf, "\\r", 2); break;
            case '\t': thrum_buf_append(buf, "\\t", 2); break;
            case '\b': thrum_buf_append(buf, "\\b", 2); break;
            case '\f': thrum_buf_append(buf, "\\f", 2); break;
            default:
                if (p[i] < 0x20) {
                    char esc[8];
                    snprintf(esc, sizeof esc, "\\u%04x", p[i]);
                    thrum_buf_append(buf, esc, 6);
                } else {
                    thrum_buf_append(buf, (const char *)&p[i], 1);
                }
        }
    }
    thrum_buf_append(buf, "\"", 1);
}

void thrum_buf_append_json_raw(thrum_buf *buf, const char *json) {
    if (json) thrum_buf_append(buf, json, strlen(json));
}
