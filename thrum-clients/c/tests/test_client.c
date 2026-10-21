/* thrum client tests — the C mirror of the helpers and protocol tests. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "thrum/client.h"
#include "thrum/json.h"
#include "thrum/identity.h"
#include "thrum/ed25519/ed25519.h"

static int failures = 0;

static void check(int condition, const char *what) {
    if (condition) {
        printf("ok   %s\n", what);
    } else {
        printf("FAIL %s\n", what);
        failures++;
    }
}

static void check_str(const char *got, const char *want, const char *what) {
    if (got && strcmp(got, want) == 0) {
        printf("ok   %s\n", what);
    } else {
        printf("FAIL %s (got %s, want %s)\n", what, got ? got : "(null)", want);
        failures++;
    }
}

static void test_sigil(void) {
    char sigil[13];
    thrum_sigil("s1", "claude-cli", sigil);
    check_str(sigil, "ff840691de5b", "sigil matches the Rust client");
    thrum_sigil("abc", "claude", sigil);
    check_str(sigil, "2ce06946efc0", "sigil is stable for another pair");
}

static void test_rid(void) {
    char rid[53];
    thrum_rid(rid);
    check(strlen(rid) == 52, "rid is 52 chars");
    check(thrum_is_valid_rid(rid), "rid is canonical Crockford base32");
    thrum_rid(rid);
    check(thrum_is_valid_rid(rid), "a second rid is canonical too");
}

static void test_json(void) {
    const char *src =
        "{\"chi\":\"chunk\",\"rid\":\"r1\",\"sid\":\"s1\",\"chunkType\":\"text_delta\","
        "\"delta\":\"hello\",\"n\":42,\"ok\":true,\"nothing\":null,\"list\":[1,2,3],"
        "\"nested\":{\"a\":\"b\"}}";
    thrum_json *value = thrum_json_parse(src, strlen(src));
    thrum_json *nested;
    thrum_json *list;

    check(value != NULL, "a tone parses");
    if (!value) return;
    check(value->pair_count == 10, "every key is read");
    check_str(thrum_json_str(thrum_json_get(value, "chi"), ""), "chunk", "chi reads");
    check_str(thrum_json_str(thrum_json_get(value, "delta"), ""), "hello", "delta reads");
    check(thrum_json_num(thrum_json_get(value, "n"), 0) == 42, "a number reads");
    check(thrum_json_bool(thrum_json_get(value, "ok"), false), "a bool reads");
    check(thrum_json_get(value, "nothing") != NULL, "null is present");
    list = thrum_json_get(value, "list");
    check(list && list->item_count == 3, "an array reads");
    nested = thrum_json_get(value, "nested");
    check(nested != NULL, "a nested object reads");
    check_str(thrum_json_str(thrum_json_get(nested, "a"), ""), "b", "a nested key reads");
    thrum_json_free(value);
}

static void test_json_escapes(void) {
    const char *src = "{\"delta\":\"say \\\"hi\\\"\\n\\ttab\"}";
    thrum_json *value = thrum_json_parse(src, strlen(src));
    check(value != NULL, "an escaped string parses");
    if (!value) return;
    check_str(thrum_json_str(thrum_json_get(value, "delta"), ""), "say \"hi\"\n\ttab",
              "escapes decode");
    thrum_json_free(value);
}

static void test_tone(void) {
    thrum_buf buf;
    thrum_buf_init(&buf);
    thrum_tone_begin(&buf, CHI_CHUNK, "r1", "s1");
    thrum_tone_str(&buf, "chunkType", "text_delta");
    thrum_tone_str(&buf, "delta", "he said \"hi\"");
    thrum_tone_int(&buf, "blockIdx", 3);
    thrum_tone_end(&buf);
    check_str(buf.data,
              "{\"chi\":\"chunk\",\"rid\":\"r1\",\"sid\":\"s1\",\"chunkType\":\"text_delta\","
              "\"delta\":\"he said \\\"hi\\\"\",\"blockIdx\":3}",
              "a tone is built with the envelope first");
    {
        thrum_json *value = thrum_json_parse(buf.data, strlen(buf.data));
        check(value != NULL, "the built tone parses back");
        if (value) {
            check_str(thrum_json_str(thrum_json_get(value, "delta"), ""), "he said \"hi\"",
                      "the built tone round-trips");
            thrum_json_free(value);
        }
    }
    thrum_buf_free(&buf);
}

static void test_socket_path(void) {
    char path[1200];
    setenv("HUM_THRUM_SOCK", "/tmp/thrum-test.sock", 1);
    check(thrum_default_socket_path(path, sizeof path), "the socket path resolves");
    check_str(path, "/tmp/thrum-test.sock", "HUM_THRUM_SOCK wins");
    unsetenv("HUM_THRUM_SOCK");
}


static void test_identity(void) {
    thrum_identity id;
    char pubkey_hex[THRUM_PUBKEY_HEX_LEN];
    char sig_hex[THRUM_SIG_HEX_LEN];
    char message[256];
    char path[1100];
    int len;

    setenv("XDG_STATE_HOME", "/tmp/thrum-identity-test", 1);
    check(thrum_identity_load_or_mint("testkind", "wbee", &id), "an identity is minted");
    check(strncmp(id.hid, "wbee_", 5) == 0, "the hid carries the role prefix");
    check(strlen(id.hid) == 69, "the hid is prefix plus 64 hex chars");

    check(thrum_identity_sign_hello(&id, 1700000000000LL, pubkey_hex, sig_hex),
          "the hello is signed");
    len = thrum_identity_socket_hello_message(id.hid, 1700000000000LL, message, sizeof message);
    check(len > 0, "the signed message is built");
    check(strncmp(message, "hum-bee-socket-v1:", 18) == 0, "the message is domain separated");
    check(strstr(message, id.hid) != NULL, "the message carries the hid");

    {
        uint8_t pubkey[32];
        uint8_t sig[64];
        size_t i;
        for (i = 0; i < 32; i++) {
            unsigned v;
            sscanf(pubkey_hex + i * 2, "%2x", &v);
            pubkey[i] = (uint8_t)v;
        }
        for (i = 0; i < 64; i++) {
            unsigned v;
            sscanf(sig_hex + i * 2, "%2x", &v);
            sig[i] = (uint8_t)v;
        }
        check(ed25519_verify(sig, (const uint8_t *)message, (size_t)len, pubkey) == 1,
              "the signature verifies");
    }

    {
        thrum_identity again;
        check(thrum_identity_load_or_mint("testkind", "wbee", &again), "the identity reloads");
        check(strcmp(again.hid, id.hid) == 0, "the identity is stable across loads");
    }
    snprintf(path, sizeof path, "/tmp/thrum-identity-test/hum/bees/testkind.key");
    check(access(path, R_OK) == 0, "the key is persisted");
}
int main(void) {
    test_sigil();
    test_rid();
    test_json();
    test_json_escapes();
    test_tone();
    test_socket_path();
    test_identity();
    if (failures) {
        printf("\n%d failure(s)\n", failures);
        return 1;
    }
    printf("\nall good\n");
    return 0;
}

