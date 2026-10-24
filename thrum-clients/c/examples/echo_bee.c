/* An echo bee: it answers every prompt with its own text. It is the
 * smallest whole bee in C, and the conformance run drives it the same
 * way it drives the echo bee in every other language.
 *
 *   ECHO_HIVE (echo-c), ECHO_AS (main), ECHO_MODEL (echo-c)
 *
 * It prints what happens to it, one line each: nestled, lost,
 * advisory <code>, then closed, refused <why> or failed <why>. A prompt
 * whose text is "stray" is answered with a key the thrum does not
 * declare; one whose text is "follow" has the bee follow its session and
 * print sid <chi> for the next tone of it. Closing its stdin closes the
 * bee.
 */

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <thrum/thrum.h>

static void say(const char *what, const char *detail) {
    printf("%s%s%s\n", what, detail ? " " : "", detail ? detail : "");
    fflush(stdout);
}

static const char *env_or(const char *key, const char *fallback) {
    const char *v = getenv(key);
    return v && v[0] ? v : fallback;
}

static const char *string_of(const cJSON *tone, const char *key) {
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(v) ? v->valuestring : "";
}

static void on_followed(thrum_bee *bee, const cJSON *tone, void *user) {
    (void)user;
    say("sid", string_of(tone, "chi"));
    thrum_bee_off_sid(bee, string_of(tone, "sid"));
}

static void on_prompt(thrum_bee *bee, const cJSON *tone, void *user) {
    (void)user;
    const char *text = string_of(tone, "text");
    const char *sid = string_of(tone, "sid");
    if (strcmp(text, "follow") == 0) thrum_bee_on_sid(bee, sid, on_followed, NULL);
    size_t n = strlen(text) + 8;
    char *delta = malloc(n);
    if (!delta) return;
    snprintf(delta, n, "echo: %s", text);

    cJSON *chunk = cJSON_CreateObject();
    cJSON_AddStringToObject(chunk, "chi", THRUM_CHI_CHUNK);
    cJSON_AddStringToObject(chunk, "sid", sid);
    cJSON_AddStringToObject(chunk, "chunkType", "text_delta");
    cJSON_AddStringToObject(chunk, "delta", delta);
    if (strcmp(text, "stray") == 0) cJSON_AddTrueToObject(chunk, "stray");
    thrum_bee_send(bee, chunk);
    cJSON_Delete(chunk);
    free(delta);

    cJSON *finish = cJSON_CreateObject();
    cJSON_AddStringToObject(finish, "chi", THRUM_CHI_FINISH);
    cJSON_AddStringToObject(finish, "sid", sid);
    cJSON_AddStringToObject(finish, "finishReason", "stop");
    thrum_bee_send(bee, finish);
    cJSON_Delete(finish);
}

static void on_nestled(thrum_bee *bee, void *user) {
    (void)bee;
    (void)user;
    say("nestled", NULL);
}

static void on_lost(thrum_bee *bee, void *user) {
    (void)bee;
    (void)user;
    say("lost", NULL);
}

static void on_advisory(thrum_bee *bee, const thrum_advisory *advisory, void *user) {
    (void)bee;
    (void)user;
    say("advisory", advisory->code);
}

static void *close_on_eof(void *arg) {
    char buf[256];
    while (read(STDIN_FILENO, buf, sizeof buf) > 0) {
    }
    thrum_bee_close(arg);
    return NULL;
}

static int report(thrum_bee *bee, int how) {
    if (how == THRUM_LINK_CLOSED) {
        say("closed", NULL);
        return 0;
    }
    say(how == THRUM_LINK_REFUSED ? "refused" : "failed", thrum_bee_error(bee));
    return how;
}

int main(void) {
    const char *model = env_or("ECHO_MODEL", "echo-c");
    cJSON *hello = cJSON_CreateObject();
    cJSON *roles = cJSON_AddArrayToObject(hello, "roles");
    cJSON_AddItemToArray(roles, cJSON_CreateString("worker"));
    cJSON_AddStringToObject(hello, "version", "1.0.0");
    cJSON *models = cJSON_AddArrayToObject(hello, "models");
    cJSON_AddItemToArray(models, cJSON_CreateString(model));
    cJSON *headroom = cJSON_AddObjectToObject(hello, "headroom");
    cJSON_AddNumberToObject(headroom, "freeSlots", 1);
    cJSON_AddNumberToObject(headroom, "totalSlots", 1);

    thrum_options opts = {
        .hive = env_or("ECHO_HIVE", "echo-c"),
        .as = env_or("ECHO_AS", "main"),
        .hello = hello,
    };
    thrum_bee *bee = thrum_bee_new(&opts);
    cJSON_Delete(hello);
    if (!bee) return 1;
    thrum_bee_on(bee, THRUM_CHI_PROMPT, on_prompt, NULL);
    thrum_bee_on_nestled(bee, on_nestled, NULL);
    thrum_bee_on_lost(bee, on_lost, NULL);
    thrum_bee_on_advisory(bee, on_advisory, NULL);

    pthread_t eof;
    pthread_create(&eof, NULL, close_on_eof, bee);
    pthread_detach(eof);

    int started = thrum_bee_start(bee);
    if (started != 0) return report(bee, started);
    return report(bee, thrum_bee_wait(bee));
}
