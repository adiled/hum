/* dwarfstar wire smoke test.
 *
 * Acts as a nestler: says hello, asks humd for one prompt, and prints
 * whatever comes back. This proves the whole path — the C client, the
 * C bee, humd's routing, and the engine's error path when no model is
 * present.
 *
 *   make test
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>

#include "thrum/client.h"

int main(int argc, char **argv) {
    thrum_client c;
    thrum_buf tone;
    char rid[53];
    const char *model = argc > 1 ? argv[1] : "ds4flash";
    const char *text = argc > 2 ? argv[2] : "say one word";
    int waited = 0;

    setvbuf(stdout, NULL, _IONBF, 0);
    thrum_client_init(&c, NULL);
    if (thrum_client_connect(&c) != 0) return 1;
    {
        struct timeval tv = {30, 0};
        setsockopt(c.fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    }
    if (thrum_client_hello(&c, "c-smoke", "0.0.1",
                           "[\"hello\",\"prompt\",\"chunk\",\"finish\",\"error\"]",
                           "{\"statefulness\":\"stateless\",\"richness\":\"lean\",\"wire\":\"thrum/c\"}") != 0) {
        printf("FAIL: hello refused\n");
        return 1;
    }
    printf("ok: hello accepted\n");

    thrum_rid(rid);
    

    thrum_buf_init(&tone);
    thrum_tone_begin(&tone, CHI_ATTACH, rid, "smoke-sid");
    thrum_tone_end(&tone);
    if (thrum_client_send(&c, &tone) != 0) return 1;
    thrum_buf_free(&tone);

    thrum_buf_init(&tone);
    thrum_tone_begin(&tone, CHI_PROMPT, rid, "smoke-sid");
    thrum_tone_str(&tone, "text", text);
    thrum_tone_str(&tone, "modelId", model);
    thrum_tone_end(&tone);
    if (thrum_client_send(&c, &tone) != 0) return 1;
    thrum_buf_free(&tone);
    printf("sent prompt (model=%s)\n", model);

    for (;;) {
        thrum_json *in = NULL;
        int got = thrum_client_recv(&c, &in);
        if (got <= 0) {
            printf("FAIL: no answer (%s)\n", got == 0 ? "eof" : "error");
            return 1;
        }
        {
            const char *chi = thrum_json_str(thrum_json_get(in, "chi"), "?");
            const char *delta = thrum_json_str(thrum_json_get(in, "delta"), NULL);
            const char *message = thrum_json_str(thrum_json_get(in, "message"), NULL);
            const char *reason = thrum_json_str(thrum_json_get(in, "finishReason"), NULL);
            printf("tone chi=%s", chi);
            if (delta) printf(" delta=\"%s\"", delta);
            if (message) printf(" message=\"%s\"", message);
            if (reason) printf(" finishReason=%s", reason);
            printf("\n");
            if (strcmp(chi, CHI_FINISH) == 0 || strcmp(chi, CHI_ERROR) == 0) {
                thrum_json_free(in);
                waited = 1;
                break;
            }
        }
        thrum_json_free(in);
    }
    thrum_client_free(&c);
    return waited ? 0 : 1;
}
