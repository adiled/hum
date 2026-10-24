// The client end of `hum bee link`, for a bee written in C.
//
// A thrum_bee runs `hum bee link` and speaks to it over two pipes, one
// JSON object per line. The link is hum's own client, in Rust: it enters
// the bee, signs the hello, reads humd's verdict, reconnects when humd
// restarts, and checks every tone against the thrum. So this file knows
// no protocol, and a C bee behaves exactly as a bee in any language.
//
//   thrum_options opts = { .hive = "dwarfstar", .as = "ds4flash", .hello = hello };
//   thrum_bee *bee = thrum_bee_new(&opts);
//   thrum_bee_on(bee, THRUM_CHI_PROMPT, on_prompt, ctx);
//   thrum_bee_on_lost(bee, on_lost, ctx);
//   if (thrum_bee_start(bee) != 0) fprintf(stderr, "%s\n", thrum_bee_error(bee));
//   int how = thrum_bee_wait(bee); // THRUM_LINK_CLOSED after thrum_bee_close
//   thrum_bee_free(bee);
//
// A tone goes to the handler for its sid if there is one, else to the
// handler for its chi, else to the any handler: an asker follows each
// session it opened by sid, a worker takes prompts by chi. Callbacks run
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
    // The bee's own hello fields, copied; NULL for none.
    const cJSON *hello;
    // The `hum` to run; thrum_hum_cli's answer when NULL.
    const char *hum;
} thrum_options;

// A bee's client; nothing runs until thrum_bee_start. NULL without memory.
thrum_bee *thrum_bee_new(const thrum_options *opts);

// Handle every tone of one chi; a later handler replaces an earlier. 0, or -1.
int thrum_bee_on(thrum_bee *bee, const char *chi, thrum_tone_fn fn, void *user);
// Handle every tone of one session, before any chi handler. 0, or -1.
int thrum_bee_on_sid(thrum_bee *bee, const char *sid, thrum_tone_fn fn, void *user);
// Stop following a session.
void thrum_bee_off_sid(thrum_bee *bee, const char *sid);
// Handle every tone no sid or chi handler takes.
void thrum_bee_on_any(thrum_bee *bee, thrum_tone_fn fn, void *user);
// humd admitted the bee: at start, and after every reconnect. 0, or -1 when full.
int thrum_bee_on_nestled(thrum_bee *bee, thrum_event_fn fn, void *user);
// humd went away. The link is dialing again; stop what humd was waiting on.
int thrum_bee_on_lost(thrum_bee *bee, thrum_event_fn fn, void *user);
// Something the bee should fix. The link has already logged it.
int thrum_bee_on_advisory(thrum_bee *bee, thrum_advisory_fn fn, void *user);

// Run the link; returns once humd has admitted the bee (0), or
// THRUM_LINK_REFUSED, or THRUM_LINK_FAILED, with thrum_bee_error saying why.
int thrum_bee_start(thrum_bee *bee);
// Send a tone; the client gives it a rid when it has none. 0, or -1.
int thrum_bee_send(thrum_bee *bee, const cJSON *tone);
// Change the hello; a field set to null is dropped. 0, or -1.
int thrum_bee_hello(thrum_bee *bee, const cJSON *patch);
// Close the link: what was sent goes out, then the link exits. Returns as thrum_bee_wait.
int thrum_bee_close(thrum_bee *bee);
// Block until the link has exited: THRUM_LINK_CLOSED after thrum_bee_close,
// else THRUM_LINK_REFUSED or THRUM_LINK_FAILED.
int thrum_bee_wait(thrum_bee *bee);

// The bee's id once the link has entered it, else "".
const char *thrum_bee_id(thrum_bee *bee);
// humd's reason when the bee was refused, why the link failed otherwise, else "".
const char *thrum_bee_error(thrum_bee *bee);
// Close the link if it runs, wait for it, and free the client.
void thrum_bee_free(thrum_bee *bee);

// Where `hum` is: $HUM_CLI, else `hum` on PATH, else ~/.local/bin/hum. 0, or -1.
int thrum_hum_cli(char *out, size_t cap);

#ifdef __cplusplus
}
#endif

#endif
