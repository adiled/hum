/* thrum client: a C bee's line to humd.
 *
 * NDJSON over a Unix stream socket: one JSON object per line. The client
 * connects, says a signed hello, waits for breath, then reads tones and
 * hands each to the handler registered for its sid, or to the wildcard.
 * Tones are cJSON objects. Sending is one locked write per line, so any
 * number of threads may send while one thread runs the read loop.
 * Lines that are not a JSON object are dropped, the way humd drops them.
 *
 *   thrum_client *c = thrum_client_new(NULL);       NULL: the default socket
 *   thrum_client_connect(c);
 *   thrum_client_hello(c, &bee, hello, NULL);       signs, sends, waits
 *   thrum_client_on_any(c, on_tone, user);
 *   thrum_client_run(c);                            until humd closes
 *
 * The hello's verdict is read the way humd gives it. humd acks every tone
 * that has a rid with chi:"echo" ok true as soon as it is delivered, so
 * the hello is admitted by the breath that follows its own ack, and is
 * refused by an echo with ok false and an error under its rid. The breath
 * humd sends on connect comes before the ack and decides nothing. A
 * refusal can come after admission, as for a revoked bee, so
 * thrum_client_run returns -1 when one arrives, with the reason in
 * thrum_error(). thrum_client_hello waits ten seconds for a verdict and
 * thrum_client_hello_within as long as it is told. thrum_client_shutdown
 * wakes a blocked run loop from another thread.
 *
 * humd restarts, and a bee outlives it. thrum_client_serve keeps a bee
 * nestled: it connects, sends the fresh hello its hook makes, signed,
 * runs, and when humd closes the socket or is not there yet, reconnects
 * with backoff (250 ms, doubling to 30 s). It returns 0 once
 * thrum_client_stop is called from anywhere, and -1 when a hello is
 * refused, which is final; a bee should then exit non-zero.
 */

#ifndef THRUM_CLIENT_H
#define THRUM_CLIENT_H

#include <stdbool.h>
#include <stddef.h>

#include <cjson/cJSON.h>

#include "identity.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*thrum_handler)(const cJSON *tone, void *user);

typedef struct thrum_client thrum_client;

thrum_client *thrum_client_new(const char *socket_path);
void thrum_client_free(thrum_client *client);
const char *thrum_client_socket_path(const thrum_client *client);

int thrum_client_connect(thrum_client *client);
void thrum_client_shutdown(thrum_client *client);

int thrum_client_send(thrum_client *client, const cJSON *tone);
int thrum_client_recv(thrum_client *client, cJSON **tone);

int thrum_client_on(thrum_client *client, const char *sid, thrum_handler fn, void *user);
void thrum_client_off(thrum_client *client, const char *sid);
void thrum_client_on_any(thrum_client *client, thrum_handler fn, void *user);
int thrum_client_run(thrum_client *client);

typedef struct {
    cJSON *(*hello)(void *user);
    void (*nestled)(void *user);
    void (*lost)(void *user, const char *why);
    void *user;
} thrum_serve_hooks;

int thrum_client_serve(thrum_client *client, const thrum_bee *bee, const thrum_serve_hooks *hooks);
void thrum_client_stop(thrum_client *client);

int thrum_client_hello(thrum_client *client, const thrum_bee *bee, cJSON *hello, cJSON **breath);
int thrum_client_hello_within(thrum_client *client, const thrum_bee *bee, cJSON *hello,
                              cJSON **breath, int timeout_ms);

cJSON *thrum_tone_new(const char *chi, const char *rid, const char *sid);

#ifdef __cplusplus
}
#endif

#endif
