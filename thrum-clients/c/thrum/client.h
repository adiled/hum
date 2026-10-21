/* thrum client — the C reference client.
 *
 * NDJSON over a Unix socket. One JSON object per line, \n separated.
 * The bee says hello, humd answers breath, then the daemon pushes
 * prompts and the bee answers with chunks and a finish.
 *
 * The wire is the contract; see WIRE.md. This file is the C mirror of
 * what the Rust, TypeScript, Python, and Go clients do — same shape,
 * same names, expressed in C: a client, handlers per sid, a wildcard
 * handler, and a run loop.
 */

#ifndef THRUM_CLIENT_H
#define THRUM_CLIENT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "chi.h"
#include "helpers.h"
#include "json.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*thrum_handler)(const thrum_json *tone, void *user);

typedef struct {
    char *sid;
    thrum_handler fn;
    void *user;
} thrum_handler_entry;

typedef struct {
    int fd;
    char socket_path[1200];
    char *rx;      
    size_t rx_len;
    size_t rx_cap;
    thrum_handler_entry *handlers;
    size_t handler_count;
    thrum_handler any;
    void *any_user;
} thrum_client;

void thrum_client_init(thrum_client *client, const char *socket_path);
void thrum_client_free(thrum_client *client);

int thrum_client_connect(thrum_client *client);

void thrum_client_close(thrum_client *client);

int thrum_client_send(thrum_client *client, const thrum_buf *tone);

int thrum_client_send_line(thrum_client *client, const char *line);

void thrum_client_on(thrum_client *client, const char *sid, thrum_handler fn, void *user);

void thrum_client_off(thrum_client *client, const char *sid);

void thrum_client_on_any(thrum_client *client, thrum_handler fn, void *user);

int thrum_client_run(thrum_client *client);

int thrum_client_recv(thrum_client *client, thrum_json **out);

int thrum_client_hello(thrum_client *client, const char *bee, const char *version,
                       const char *chis_json, const char *propensity_json);

void thrum_tone_begin(thrum_buf *buf, const char *chi, const char *rid, const char *sid);

void thrum_tone_str(thrum_buf *buf, const char *key, const char *value);

void thrum_tone_strn(thrum_buf *buf, const char *key, const char *value, size_t len);

void thrum_tone_strn(thrum_buf *buf, const char *key, const char *value, size_t len);

void thrum_tone_int(thrum_buf *buf, const char *key, int64_t value);

void thrum_tone_raw(thrum_buf *buf, const char *key, const char *json);

void thrum_tone_end(thrum_buf *buf);

#ifdef __cplusplus
}
#endif

#endif 
