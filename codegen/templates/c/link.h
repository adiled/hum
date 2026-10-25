// The client end of `hum bee link`, for a bee written in C.
//
// A thrum_bee runs `hum bee link` and speaks to it over two pipes, one
// JSON object per line. The link is hum's own client, in Rust: it enters
// the bee for its role, signs the hello, reads humd's verdict, reconnects
// when humd restarts, and checks every tone against the thrum. So this
// file knows no protocol, and a C bee behaves exactly as a bee in any
// language.
//
//   thrum_options opts = { .hive = "dwarfstar", .as = "ds4flash", .role = THRUM_ROLE_NURSE, .hello = hello };
//   thrum_bee *bee = thrum_bee_new(&opts);
//   thrum_bee_on(bee, THRUM_CHI_PROMPT, on_prompt, ctx);
//   thrum_bee_on_astray(bee, on_astray, ctx);
//   if (thrum_bee_start(bee) != 0) fprintf(stderr, "%s\n", thrum_bee_error(bee));
//   int how = thrum_bee_wait(bee); // THRUM_LINK_CLOSED after thrum_bee_close
//   thrum_bee_free(bee);
//
// A tone goes to the handler for its sid if there is one, else to the
// handler for its chi, else to the any handler: a forager follows each
// sigil it opened by sid, a nurse takes prompts by chi. Callbacks run
// one at a time, in the order tones arrive, on the thread
// reading the link; a tone is valid only during its callback. Hand long
// work to a thread of its own, so the tone after it (a cancel) is not
// held up. thrum_bee_send is safe from any thread, a callback's too;
// thrum_bee_close, thrum_bee_wait and thrum_bee_free are not for a
// callback, since they wait for the thread that runs it. A tone sent while
// humd is away is dropped by the link, which says so. If SIGPIPE is at
// its default, starting a bee ignores it, so writing to a link that has
// gone returns an error instead of killing the bee.

#ifndef THRUM_LINK_H
#define THRUM_LINK_H

#include <stddef.h>

#include <cjson/cJSON.h>

#ifdef __cplusplus
extern "C" {
#endif

// @constants

typedef struct thrum_bee thrum_bee;

typedef struct {
    const char *code;
    const char *message;
    const char *from;
    const cJSON *fields;
} thrum_advisory;

typedef void (*thrum_tone_fn)(thrum_bee *bee, const cJSON *tone, void *user);
typedef void (*thrum_event_fn)(thrum_bee *bee, void *user);
typedef void (*thrum_advisory_fn)(thrum_bee *bee, const thrum_advisory *advisory, void *user);

typedef struct {
    const char *hive;
    const char *as;
    const char *role;
    const cJSON *hello;
    const char *hum;
} thrum_options;

thrum_bee *thrum_bee_new(const thrum_options *opts);

int thrum_bee_on(thrum_bee *bee, const char *chi, thrum_tone_fn fn, void *user);
int thrum_bee_on_sid(thrum_bee *bee, const char *sid, thrum_tone_fn fn, void *user);
void thrum_bee_off_sid(thrum_bee *bee, const char *sid);
void thrum_bee_on_any(thrum_bee *bee, thrum_tone_fn fn, void *user);
int thrum_bee_on_nestled(thrum_bee *bee, thrum_event_fn fn, void *user);
int thrum_bee_on_astray(thrum_bee *bee, thrum_event_fn fn, void *user);
int thrum_bee_on_advisory(thrum_bee *bee, thrum_advisory_fn fn, void *user);

int thrum_bee_start(thrum_bee *bee);
int thrum_bee_send(thrum_bee *bee, const cJSON *tone);
int thrum_bee_hello(thrum_bee *bee, const cJSON *patch);
int thrum_bee_close(thrum_bee *bee);
int thrum_bee_wait(thrum_bee *bee);

const char *thrum_bee_id(thrum_bee *bee);
const char *thrum_bee_error(thrum_bee *bee);
void thrum_bee_free(thrum_bee *bee);

int thrum_hum_cli(char *out, size_t cap);

#ifdef __cplusplus
}
#endif

#endif
