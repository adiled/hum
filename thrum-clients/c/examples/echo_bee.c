/* echo_bee: the smallest C worker bee.
 *
 * It enters as <label> under <hive> through the hum CLI, signs its hello,
 * and answers every prompt with its own text as one text_delta chunk and
 * a finish. The hive must be installed on the humd it dials.
 *
 *   echo_bee <hive> [label]
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <string.h>

#include <thrum/thrum.h>

static const char *string_of(const cJSON *tone, const char *key) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static cJSON *reply(const cJSON *prompt, const char *chi) {
    const char *rid = string_of(prompt, "rid");
    return thrum_tone_new(chi, rid, string_of(prompt, "sid"));
}

static void answer(thrum_client *client, const cJSON *prompt) {
    const char *text = string_of(prompt, "text");
    cJSON *chunk = reply(prompt, THRUM_CHI_CHUNK);
    cJSON *finish = reply(prompt, THRUM_CHI_FINISH);

    if (chunk && finish) {
        cJSON_AddStringToObject(chunk, "chunkType", "text_delta");
        cJSON_AddStringToObject(chunk, "delta", text ? text : "");
        cJSON_AddStringToObject(finish, "finishReason", "stop");
        if (thrum_client_send(client, chunk) != 0 || thrum_client_send(client, finish) != 0) {
            fprintf(stderr, "echo_bee: %s\n", thrum_error());
        }
    }
    cJSON_Delete(chunk);
    cJSON_Delete(finish);
}

static void on_tone(const cJSON *tone, void *user) {
    const char *chi = string_of(tone, "chi");
    if (chi && strcmp(chi, THRUM_CHI_PROMPT) == 0) answer((thrum_client *)user, tone);
}

static cJSON *hello_for(const char *hive) {
    static const char *const chis[] = {THRUM_CHI_HELLO, THRUM_CHI_PROMPT, THRUM_CHI_CHUNK,
                                       THRUM_CHI_FINISH};
    static const char *const roles[] = {"worker"};
    static const char *const models[] = {"echo"};
    cJSON *hello = cJSON_CreateObject();
    cJSON *propensity = cJSON_AddObjectToObject(hello, "propensity");

    cJSON_AddStringToObject(hello, "hive", hive);
    cJSON_AddItemToObject(hello, "roles", cJSON_CreateStringArray(roles, 1));
    cJSON_AddStringToObject(hello, "version", "0.1.0");
    cJSON_AddItemToObject(hello, "chis", cJSON_CreateStringArray(chis, 4));
    cJSON_AddItemToObject(hello, "models", cJSON_CreateStringArray(models, 1));
    cJSON_AddStringToObject(propensity, "statefulness", "stateless");
    cJSON_AddStringToObject(propensity, "richness", "lean");
    cJSON_AddStringToObject(propensity, "wire", "echo");
    return hello;
}

int main(int argc, char **argv) {
    const char *hive = argc > 1 ? argv[1] : "echo";
    const char *label = argc > 2 ? argv[2] : "main";
    thrum_bee bee;
    thrum_client *client;
    cJSON *hello;
    int rc;

    if (thrum_bee_enter_as(hive, label, &bee) != 0) {
        fprintf(stderr, "echo_bee: %s\n", thrum_error());
        return 1;
    }
    client = thrum_client_new(NULL);
    if (!client || thrum_client_connect(client) != 0) {
        fprintf(stderr, "echo_bee: %s\n", thrum_error());
        thrum_client_free(client);
        return 1;
    }
    hello = hello_for(hive);
    rc = thrum_client_hello(client, &bee, hello, NULL);
    cJSON_Delete(hello);
    thrum_bee_forget(&bee);
    if (rc != 0) {
        fprintf(stderr, "echo_bee: %s\n", thrum_error());
        thrum_client_free(client);
        return 1;
    }
    fprintf(stderr, "echo_bee: nestled under %s on %s\n", hive, thrum_client_socket_path(client));

    thrum_client_on_any(client, on_tone, client);
    rc = thrum_client_run(client);
    if (rc < 0) fprintf(stderr, "echo_bee: %s\n", thrum_error());
    thrum_client_free(client);
    return rc < 0 ? 1 : 0;
}
