/* test_thrum: the C client against the wire it mirrors.
 *
 * The vectors are not this client's own output. The HumId ones are
 * hum-identity's ts_parity_vectors, the same the Go and Python clients
 * check; the sigils, the bee id and the hello signature were produced by
 * thrum_core::sigil and hum_identity::BeeKey over the RFC 8032 test-1
 * seed. The golden fixture is thrum-core's. A thread plays humd over a
 * real Unix socket: it verifies hellos with libsodium on its own, refuses
 * them the way humd does, and reads concurrent sends line by line.
 *
 *   test_thrum <golden.ndjson>
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <sodium.h>
#include <thrum/thrum.h>

static int checks;
static int failures;

#define CHECK(cond)                                                                \
    do {                                                                           \
        checks++;                                                                  \
        if (!(cond)) {                                                             \
            failures++;                                                            \
            fprintf(stderr, "%s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #cond); \
        }                                                                          \
    } while (0)

#define CHECK_STR(got, want)                                                               \
    do {                                                                                   \
        const char *got_ = (got);                                                          \
        const char *want_ = (want);                                                        \
        checks++;                                                                          \
        if (!got_ || strcmp(got_, want_) != 0) {                                           \
            failures++;                                                                    \
            fprintf(stderr, "%s:%d: got \"%s\", want \"%s\"\n", __FILE__, __LINE__,        \
                    got_ ? got_ : "(null)", want_);                                        \
        }                                                                                  \
    } while (0)

#define CHECK_HAS(haystack, needle)                                                       \
    do {                                                                                  \
        const char *hay_ = (haystack);                                                    \
        checks++;                                                                         \
        if (!hay_ || !strstr(hay_, (needle))) {                                           \
            failures++;                                                                   \
            fprintf(stderr, "%s:%d: \"%s\" does not mention \"%s\"\n", __FILE__, __LINE__, \
                    hay_ ? hay_ : "(null)", (needle));                                    \
        }                                                                                 \
    } while (0)

static const char RFC8032_SEED[] = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
static const char RFC8032_PUBKEY[] = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
static const char RUST_BEE_ID[] = "bee_21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9";
static const char RUST_SIGNATURE_AT_1700000000000[] =
    "349c034710720af73dd18be0b8fd741ede7b7e70eaec08f6bd414b57158cda47"
    "da623343d3a2b54733c8d9b711d528adbe19724f6f75e9cb5c820424a40c3001";

static char tmp_root[512];

static void path_in(char *out, size_t cap, const char *rel) {
    snprintf(out, cap, "%s/%s", tmp_root, rel);
}

static void mkdirs(const char *path) {
    char buf[1024];
    char *p;
    snprintf(buf, sizeof buf, "%s", path);
    for (p = buf + 1; *p; p++) {
        if (*p == '/') {
            *p = '\0';
            mkdir(buf, 0700);
            *p = '/';
        }
    }
    mkdir(buf, 0700);
}

static void write_file(const char *path, const void *bytes, size_t n, mode_t mode) {
    char dir[1024];
    char *slash;
    FILE *f;
    snprintf(dir, sizeof dir, "%s", path);
    slash = strrchr(dir, '/');
    if (slash) {
        *slash = '\0';
        mkdirs(dir);
    }
    f = fopen(path, "wb");
    if (!f) {
        fprintf(stderr, "cannot write %s: %s\n", path, strerror(errno));
        exit(2);
    }
    fwrite(bytes, 1, n, f);
    fclose(f);
    chmod(path, mode);
}

static void rfc_seed(unsigned char seed[32]) {
    sodium_hex2bin(seed, 32, RFC8032_SEED, 64, NULL, NULL, NULL);
}

static const char *str_of(const cJSON *tone, const char *key) {
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(v) ? v->valuestring : NULL;
}

typedef struct {
    int fd;
    char *buf;
    size_t start;
    size_t len;
    size_t cap;
} line_reader;

static char *read_line(line_reader *r) {
    for (;;) {
        char *nl = r->len > r->start ? memchr(r->buf + r->start, '\n', r->len - r->start) : NULL;
        ssize_t n;
        if (nl) {
            size_t len = (size_t)(nl - (r->buf + r->start));
            char *line = malloc(len + 1);
            memcpy(line, r->buf + r->start, len);
            line[len] = '\0';
            r->start += len + 1;
            return line;
        }
        if (r->start > 0) {
            memmove(r->buf, r->buf + r->start, r->len - r->start);
            r->len -= r->start;
            r->start = 0;
        }
        if (r->cap - r->len < 65536) {
            r->cap = r->cap ? r->cap * 2 : 131072;
            r->buf = realloc(r->buf, r->cap);
        }
        do {
            n = read(r->fd, r->buf + r->len, r->cap - r->len);
        } while (n < 0 && errno == EINTR);
        if (n <= 0) return NULL;
        r->len += (size_t)n;
    }
}

static void send_text(int fd, const char *text) {
    size_t n = strlen(text);
    while (n > 0) {
        ssize_t w = write(fd, text, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) return;
        text += w;
        n -= (size_t)w;
    }
}

static int listen_at(const char *path) {
    struct sockaddr_un addr;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    unlink(path);
    memset(&addr, 0, sizeof addr);
    addr.sun_family = AF_UNIX;
    snprintf(addr.sun_path, sizeof addr.sun_path, "%s", path);
    if (fd < 0 || bind(fd, (struct sockaddr *)&addr, sizeof addr) != 0 || listen(fd, 4) != 0) {
        fprintf(stderr, "cannot listen on %s: %s\n", path, strerror(errno));
        exit(2);
    }
    return fd;
}

typedef struct {
    int listen_fd;
    const char *script;
    const char *text;
    int lines;
    int bad;
    int verdict;
    char why[256];
} humd_play;

static void *humd_writes(void *arg) {
    humd_play *play = arg;
    int fd = accept(play->listen_fd, NULL, NULL);
    size_t n = strlen(play->text);
    size_t off;
    for (off = 0; off < n; off += 7) {
        char piece[8];
        size_t take = n - off < 7 ? n - off : 7;
        memcpy(piece, play->text + off, take);
        piece[take] = '\0';
        send_text(fd, piece);
    }
    shutdown(fd, SHUT_WR);
    close(fd);
    return NULL;
}

static void *humd_reads(void *arg) {
    humd_play *play = arg;
    line_reader r = {0};
    char *line;
    r.fd = accept(play->listen_fd, NULL, NULL);
    while ((line = read_line(&r)) != NULL) {
        cJSON *tone = cJSON_Parse(line);
        const cJSON *delta = cJSON_GetObjectItemCaseSensitive(tone, "delta");
        play->lines++;
        if (!cJSON_IsObject(tone) || !cJSON_IsString(delta) || strlen(delta->valuestring) != 20000) {
            play->bad++;
        }
        cJSON_Delete(tone);
        free(line);
    }
    free(r.buf);
    close(r.fd);
    return NULL;
}

static int verify_hello(const char *line, char *why, size_t cap) {
    cJSON *hello = cJSON_Parse(line);
    const char *hid = str_of(hello, "hid");
    const char *pubkey_hex = str_of(hello, "pubkey");
    const char *signature_hex = str_of(hello, "signature");
    const cJSON *signed_at = cJSON_GetObjectItemCaseSensitive(hello, "signedAt");
    unsigned char pubkey[32];
    unsigned char signature[64];
    unsigned char hash[32];
    char id[69];
    char message[200];
    int n;
    int rc = -1;

    if (!hello) {
        snprintf(why, cap, "not JSON");
    } else if (!str_of(hello, "chi") || strcmp(str_of(hello, "chi"), "hello") != 0) {
        snprintf(why, cap, "not a hello");
    } else if (!thrum_is_valid_rid(str_of(hello, "rid"))) {
        snprintf(why, cap, "no canonical rid");
    } else if (!str_of(hello, "protoVersion") || strcmp(str_of(hello, "protoVersion"), THRUM_VERSION) != 0) {
        snprintf(why, cap, "protoVersion is not THRUM_VERSION");
    } else if (thrum_tone_undeclared(hello, NULL, 0) != 0) {
        snprintf(why, cap, "the hello carries an undeclared key");
    } else if (!hid || !pubkey_hex || !signature_hex || !cJSON_IsNumber(signed_at)) {
        snprintf(why, cap, "unsigned");
    } else if (!str_of(hello, "from") || strcmp(str_of(hello, "from"), hid) != 0) {
        snprintf(why, cap, "from is not the hid");
    } else if (sodium_hex2bin(pubkey, 32, pubkey_hex, strlen(pubkey_hex), NULL, NULL, NULL) != 0 ||
               sodium_hex2bin(signature, 64, signature_hex, strlen(signature_hex), NULL, NULL, NULL) != 0) {
        snprintf(why, cap, "bad hex");
    } else {
        crypto_hash_sha256(hash, pubkey, 32);
        memcpy(id, "bee_", 4);
        sodium_bin2hex(id + 4, sizeof id - 4, hash, 32);
        n = snprintf(message, sizeof message, "hum-bee-socket-v1:%s:%lld", hid,
                     (long long)signed_at->valuedouble);
        if (strcmp(id, hid) != 0) {
            snprintf(why, cap, "hid is not sha256 of the pubkey");
        } else if (crypto_sign_verify_detached(signature, (const unsigned char *)message,
                                               (unsigned long long)n, pubkey) != 0) {
            snprintf(why, cap, "signature does not verify");
        } else {
            rc = 0;
        }
    }
    cJSON_Delete(hello);
    return rc;
}

static void send_echo(int fd, const char *rid, const char *error) {
    cJSON *echo = cJSON_CreateObject();
    char *text;
    cJSON_AddStringToObject(echo, "chi", "echo");
    cJSON_AddStringToObject(echo, "rid", rid);
    cJSON_AddBoolToObject(echo, "ok", error == NULL);
    if (error) cJSON_AddStringToObject(echo, "error", error);
    text = cJSON_PrintUnformatted(echo);
    send_text(fd, text);
    send_text(fd, "\n");
    cJSON_free(text);
    cJSON_Delete(echo);
}

static void drain_until_closed(line_reader *r) {
    char *line;
    while ((line = read_line(r)) != NULL) free(line);
}

static void *humd_hello(void *arg) {
    humd_play *play = arg;
    line_reader r = {0};
    char *line;
    char rid[128] = "";
    r.fd = accept(play->listen_fd, NULL, NULL);
    send_text(r.fd, "{\"chi\":\"breath\",\"from\":\"daemon\",\"sessions\":[]}\n");
    line = read_line(&r);
    if (line && strcmp(play->script, "hang-up") != 0) {
        cJSON *hello = cJSON_Parse(line);
        snprintf(rid, sizeof rid, "%s", str_of(hello, "rid") ? str_of(hello, "rid") : "");
        cJSON_Delete(hello);
        play->verdict = verify_hello(line, play->why, sizeof play->why);
        send_text(r.fd, "{\"chi\":\"pulse\",\"rid\":\"p\",\"sid\":\"early\",\"kind\":\"status\"}\n");
        send_echo(r.fd, rid, NULL);
        if (strcmp(play->script, "admit") == 0) {
            send_text(r.fd, "{\"chi\":\"breath\",\"from\":\"daemon\",\"sessions\":{}}\n");
        } else if (strcmp(play->script, "refuse") == 0) {
            send_echo(r.fd, "someone-else", "not yours");
            send_echo(r.fd, rid, "bee has not entered under this hive");
            drain_until_closed(&r);
        } else if (strcmp(play->script, "revoke") == 0) {
            send_text(r.fd, "{\"chi\":\"breath\",\"from\":\"daemon\",\"sessions\":{}}\n");
            send_text(r.fd, "{\"chi\":\"prompt\",\"rid\":\"q\",\"sid\":\"s\",\"text\":\"hi\"}\n");
            send_echo(r.fd, rid, "bee identity revoked");
            drain_until_closed(&r);
        } else if (strcmp(play->script, "silent") == 0) {
            drain_until_closed(&r);
        }
    }
    free(line);
    free(r.buf);
    close(r.fd);
    return NULL;
}

static void the_humid_encoder_matches_hum_identity(void) {
    uint8_t zero[32] = {0};
    uint8_t ts[32] = {0};
    uint8_t ones[32];
    uint64_t ms = 1700000000000ULL;
    char out[THRUM_RID_LEN];
    char want[THRUM_RID_LEN];
    int i;

    thrum_humid_encode(zero, out);
    memset(want, '0', 52);
    want[52] = '\0';
    CHECK_STR(out, want);

    for (i = 0; i < 6; i++) ts[5 - i] = (uint8_t)(ms >> (8 * i));
    thrum_humid_encode(ts, out);
    memcpy(want, "065WZSB8", 8);
    memset(want + 8, '0', 44);
    CHECK_STR(out, want);

    memset(ones, 0xff, sizeof ones);
    thrum_humid_encode(ones, out);
    memset(want, 'Z', 51);
    want[51] = 'G';
    CHECK_STR(out, want);
}

static void a_rid_is_52_crockford_and_garbage_is_not(void) {
    char a[THRUM_RID_LEN];
    char b[THRUM_RID_LEN];
    char eye[53];
    int i;
    for (i = 0; i < 100; i++) {
        thrum_rid(a);
        CHECK(thrum_is_valid_rid(a));
        CHECK(strlen(a) == 52);
    }
    thrum_rid(b);
    CHECK(strcmp(a, b) != 0);
    memset(eye, 'I', 52);
    eye[52] = '\0';
    CHECK(!thrum_is_valid_rid(""));
    CHECK(!thrum_is_valid_rid(eye));
    CHECK(!thrum_is_valid_rid("000000000000000000000000000000000000000000000000000"));
    CHECK(!thrum_is_valid_rid(NULL));
}

static void the_sigil_matches_thrum_core(void) {
    char out[THRUM_SIGIL_LEN];
    thrum_sigil("abc", "claude", out);
    CHECK_STR(out, "2ce06946efc0");
    thrum_sigil("sess-1", "nest-a", out);
    CHECK_STR(out, "afc65fe81bc3");
}

static void the_registry_is_the_generated_one(void) {
    size_t i;
    CHECK(strlen(THRUM_VERSION) > 0);
    CHECK(thrum_is_valid_chi(THRUM_CHI_HELLO));
    CHECK(thrum_is_valid_chi("tool-call"));
    CHECK(!thrum_is_valid_chi("nonsense"));
    CHECK(!thrum_is_valid_chi(NULL));
    CHECK(THRUM_CHI_COUNT == THRUM_VIEW_COUNT);
    for (i = 0; i < THRUM_CHI_COUNT; i++) CHECK(thrum_view_of(THRUM_ALL_CHI[i]) != NULL);
    for (i = 0; i < THRUM_PULSE_COUNT; i++) CHECK(thrum_is_valid_pulse(THRUM_ALL_PULSE[i]));
    CHECK(thrum_view_of("nonsense") == NULL);
}

static bool carries_required(const cJSON *tone, const thrum_view *view, const char **missing) {
    size_t i;
    for (i = 0; i < view->count; i++) {
        if (!view->fields[i].optional &&
            !cJSON_GetObjectItemCaseSensitive(tone, view->fields[i].json)) {
            *missing = view->fields[i].json;
            return false;
        }
    }
    return true;
}

static void the_golden_fixture_reads_through_the_socket(const char *golden) {
    char sock[600];
    size_t len = 0;
    char *fixture;
    char *text;
    humd_play play = {0};
    pthread_t humd;
    thrum_client *client;
    cJSON *tone;
    int golden_lines = 0;
    int got = 0;
    int rc;
    const char *p;
    static const char junk[] =
        "not json\n"
        "[1,2]\n"
        "\n"
        "   \n"
        "{\"chi\":\"echo\",\"rid\":\"g\",\"ok\":true} trailing\n"
        "{\"chi\":\"echo\",\"rid\":\"crlf\",\"ok\":true}\r\n"
        "{\"chi\":\"echo\",\"rid\":\"last\",\"ok\":true}";

    {
        FILE *f = fopen(golden, "rb");
        if (!f) {
            fprintf(stderr, "cannot open the golden fixture %s\n", golden);
            failures++;
            return;
        }
        fseek(f, 0, SEEK_END);
        len = (size_t)ftell(f);
        fseek(f, 0, SEEK_SET);
        fixture = malloc(len + 1);
        len = fread(fixture, 1, len, f);
        fixture[len] = '\0';
        fclose(f);
    }
    for (p = fixture; *p; p++) {
        if (*p == '\n' && p > fixture && p[-1] != '\n') golden_lines++;
    }
    text = malloc(len + sizeof junk + 1);
    memcpy(text, fixture, len);
    memcpy(text + len, junk, sizeof junk);

    path_in(sock, sizeof sock, "golden.sock");
    play.listen_fd = listen_at(sock);
    play.text = text;
    pthread_create(&humd, NULL, humd_writes, &play);

    client = thrum_client_new(sock);
    CHECK(client != NULL);
    CHECK(thrum_client_connect(client) == 0);
    while ((rc = thrum_client_recv(client, &tone)) == 1) {
        const char *chi = str_of(tone, "chi");
        const char *stray[4];
        const char *missing = NULL;
        const thrum_view *view = thrum_view_of(chi);
        got++;
        CHECK(thrum_is_valid_chi(chi));
        CHECK(view != NULL);
        CHECK(thrum_tone_undeclared(tone, stray, 4) == 0);
        if (view && got <= golden_lines) {
            if (!carries_required(tone, view, &missing) ||
                !carries_required(tone, &THRUM_ENVELOPE, &missing)) {
                failures++;
                fprintf(stderr, "golden %s lacks required %s\n", chi, missing);
            }
        }
        if (got == golden_lines + 1) CHECK_STR(str_of(tone, "rid"), "crlf");
        if (got == golden_lines + 2) CHECK_STR(str_of(tone, "rid"), "last");
        cJSON_Delete(tone);
    }
    CHECK(rc == 0);
    CHECK(golden_lines > 10);
    CHECK(got == golden_lines + 2);

    pthread_join(humd, NULL);
    close(play.listen_fd);
    thrum_client_free(client);
    free(text);
    free(fixture);
}

static void undeclared_keys_are_named_as_humd_names_them(void) {
    cJSON *stray_hello = cJSON_Parse("{\"chi\":\"hello\",\"rid\":\"r\",\"hive\":\"h\",\"toolNames\":[\"a\"]}");
    cJSON *cancel = cJSON_Parse("{\"chi\":\"cancel\",\"rid\":\"r\",\"sid\":\"s\",\"ext\":{\"mine\":{\"x\":1}}}");
    cJSON *nonsense = cJSON_Parse("{\"chi\":\"nonsense\",\"x\":1}");
    cJSON *string = cJSON_Parse("\"not an object\"");
    const char *stray[4] = {0};

    CHECK(thrum_tone_undeclared(stray_hello, stray, 4) == 1);
    CHECK_STR(stray[0], "toolNames");
    CHECK(thrum_tone_undeclared(cancel, stray, 4) == 0);
    CHECK(thrum_tone_undeclared(nonsense, stray, 4) == 0);
    CHECK(thrum_tone_undeclared(string, stray, 4) == 0);
    CHECK(thrum_key_is_declared("hello", "signedAt"));
    CHECK(thrum_key_is_declared("chunk", "sentAt"));
    CHECK(!thrum_key_is_declared("hello", "toolNames"));
    CHECK(!thrum_key_is_declared("nonsense", "rid"));

    cJSON_Delete(stray_hello);
    cJSON_Delete(cancel);
    cJSON_Delete(nonsense);
    cJSON_Delete(string);
}

static void a_seed_is_the_bee_hum_identity_derives(void) {
    unsigned char seed[32];
    char pubkey_hex[65];
    thrum_bee bee;

    rfc_seed(seed);
    CHECK(thrum_bee_from_seed(seed, &bee) == 0);
    sodium_bin2hex(pubkey_hex, sizeof pubkey_hex, bee.pubkey, 32);
    CHECK_STR(pubkey_hex, RFC8032_PUBKEY);
    CHECK_STR(bee.id, RUST_BEE_ID);
    CHECK(thrum_bee_id_is_valid(bee.id));
    thrum_bee_forget(&bee);
    CHECK(bee.id[0] == '\0');
}

static void the_hello_signature_is_the_one_hum_identity_makes(void) {
    unsigned char seed[32];
    thrum_bee bee;
    cJSON *hello = cJSON_CreateObject();
    char *printed;
    char message[160];

    rfc_seed(seed);
    thrum_bee_from_seed(seed, &bee);
    CHECK(thrum_bee_hello_message(bee.id, 1700000000000LL, message, sizeof message) > 0);
    CHECK_STR(message, "hum-bee-socket-v1:bee_21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9:1700000000000");

    cJSON_AddStringToObject(hello, "hid", "stale");
    CHECK(thrum_bee_sign_hello_at(&bee, hello, 1700000000000LL) == 0);
    CHECK_STR(str_of(hello, "signature"), RUST_SIGNATURE_AT_1700000000000);
    CHECK_STR(str_of(hello, "hid"), RUST_BEE_ID);
    CHECK_STR(str_of(hello, "from"), RUST_BEE_ID);
    CHECK_STR(str_of(hello, "pubkey"), RFC8032_PUBKEY);
    printed = cJSON_PrintUnformatted(hello);
    CHECK_HAS(printed, "\"signedAt\":1700000000000,");
    CHECK(strstr(printed, "\"hid\":\"stale\"") == NULL);
    cJSON_free(printed);

    CHECK(thrum_bee_sign_hello(&bee, hello) == 0);
    {
        const cJSON *at = cJSON_GetObjectItemCaseSensitive(hello, "signedAt");
        char again[160];
        unsigned char signature[64];
        int n = thrum_bee_hello_message(bee.id, (int64_t)at->valuedouble, again, sizeof again);
        sodium_hex2bin(signature, 64, str_of(hello, "signature"), 128, NULL, NULL, NULL);
        CHECK(crypto_sign_verify_detached(signature, (const unsigned char *)again,
                                          (unsigned long long)n, bee.pubkey) == 0);
    }
    cJSON_Delete(hello);
}

static void a_filed_key_loads_only_as_its_own_id(void) {
    unsigned char seed[32];
    char state[600];
    char path[800];
    char liar[] = "bee_0000000000000000000000000000000000000000000000000000000000000000";
    char upper[sizeof RUST_BEE_ID];
    thrum_bee bee;
    size_t i;

    rfc_seed(seed);
    path_in(state, sizeof state, "state");
    setenv("XDG_STATE_HOME", state, 1);

    CHECK(thrum_bee_key_path(RUST_BEE_ID, path, sizeof path) == 0);
    CHECK_HAS(path, "/hum/bees/bee_21fe31df");
    write_file(path, seed, 32, 0600);
    CHECK(thrum_bee_load(RUST_BEE_ID, &bee) == 0);
    CHECK_STR(bee.id, RUST_BEE_ID);

    CHECK(thrum_bee_key_path(liar, path, sizeof path) == 0);
    write_file(path, seed, 32, 0600);
    CHECK(thrum_bee_load(liar, &bee) != 0);
    CHECK_HAS(thrum_error(), "is not the key of");

    write_file(path, seed, 31, 0600);
    CHECK(thrum_bee_load(liar, &bee) != 0);
    CHECK_HAS(thrum_error(), "expected 32");

    snprintf(upper, sizeof upper, "%s", RUST_BEE_ID);
    for (i = 4; upper[i]; i++) {
        if (upper[i] >= 'a' && upper[i] <= 'f') upper[i] = (char)(upper[i] - 'a' + 'A');
    }
    CHECK(!thrum_bee_id_is_valid("../humd"));
    CHECK(!thrum_bee_id_is_valid(""));
    CHECK(!thrum_bee_id_is_valid(upper));
    CHECK(!thrum_bee_id_is_valid("wbee_21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9"));
    CHECK(thrum_bee_load("../humd", &bee) != 0);
    CHECK_HAS(thrum_error(), "is not a bee id");

    CHECK(thrum_is_plain_name("dwarfstar"));
    CHECK(thrum_is_plain_name("requester-ab12_c.d"));
    CHECK(!thrum_is_plain_name(".hidden"));
    CHECK(!thrum_is_plain_name("a b"));
    CHECK(!thrum_is_plain_name("a/b"));
    CHECK(!thrum_is_plain_name(""));
}

static void entering_goes_through_the_hum_cli(void) {
    char bin[600];
    char hum[700];
    char script[1024];
    char path_env[4096];
    const char *old_path = getenv("PATH");
    char *saved = old_path ? strdup(old_path) : NULL;
    char id[THRUM_BEE_ID_LEN];
    thrum_bee bee;

    path_in(bin, sizeof bin, "bin");
    snprintf(hum, sizeof hum, "%s/hum", bin);
    snprintf(script, sizeof script,
             "#!/bin/sh\n"
             "if [ \"$1 $2 $3 $5\" = \"bee enter --hive --as\" ] && [ $# -eq 6 ]; then\n"
             "  case \"$4:$6\" in\n"
             "    ctest:main) echo \"  %s  \"; exit 0 ;;\n"
             "    ctest:junk) echo \"hello there\"; exit 0 ;;\n"
             "    ctest:fail) exit 4 ;;\n"
             "  esac\n"
             "fi\n"
             "exit 9\n",
             RUST_BEE_ID);
    write_file(hum, script, strlen(script), 0755);
    snprintf(path_env, sizeof path_env, "%s:%s", bin, saved ? saved : "/usr/bin:/bin");
    setenv("PATH", path_env, 1);

    CHECK(thrum_bee_enter("ctest", "main", id) == 0);
    CHECK_STR(id, RUST_BEE_ID);
    CHECK(thrum_bee_enter_as("ctest", "main", &bee) == 0);
    CHECK_STR(bee.id, RUST_BEE_ID);

    CHECK(thrum_bee_enter("ctest", "junk", id) != 0);
    CHECK_HAS(thrum_error(), "not a bee id");
    CHECK(thrum_bee_enter("ctest", "fail", id) != 0);
    CHECK_HAS(thrum_error(), "status 4");
    CHECK(thrum_bee_enter("ctest", "a b", id) != 0);
    CHECK_HAS(thrum_error(), "not a plain label");
    CHECK(thrum_bee_enter("../ctest", "main", id) != 0);
    CHECK_HAS(thrum_error(), "not a plain hive name");

    if (saved) {
        setenv("PATH", saved, 1);
        free(saved);
    }
}

static void the_socket_resolves_like_hum_paths(void) {
    char state[600];
    char want[800];
    char got[1024];
    char rt[800];
    char home[600];
    const char *old_home = getenv("HOME");
    char *saved_home = old_home ? strdup(old_home) : NULL;
    static const char runtime[] = "{\"socket\":\"/run/rendezvous/thrum.sock\",\"pid\":1}";

    unsetenv("HUM_THRUM_SOCK");
    unsetenv("HUM_SOCKET");
    path_in(state, sizeof state, "sockstate");
    setenv("XDG_STATE_HOME", state, 1);

    snprintf(want, sizeof want, "%s/hum/thrum.sock", state);
    CHECK(thrum_default_socket_path(got, sizeof got) == 0);
    CHECK_STR(got, want);

    snprintf(rt, sizeof rt, "%s/hum/runtime.json", state);
    write_file(rt, runtime, strlen(runtime), 0600);
    CHECK(thrum_default_socket_path(got, sizeof got) == 0);
    CHECK_STR(got, "/run/rendezvous/thrum.sock");

    setenv("HUM_SOCKET", "/from/hum_socket", 1);
    CHECK(thrum_default_socket_path(got, sizeof got) == 0);
    CHECK_STR(got, "/from/hum_socket");
    setenv("HUM_THRUM_SOCK", "/from/hum_thrum_sock", 1);
    CHECK(thrum_default_socket_path(got, sizeof got) == 0);
    CHECK_STR(got, "/from/hum_thrum_sock");
    unsetenv("HUM_THRUM_SOCK");
    unsetenv("HUM_SOCKET");

    unsetenv("XDG_STATE_HOME");
    path_in(home, sizeof home, "home");
    setenv("HOME", home, 1);
    snprintf(want, sizeof want, "%s/.local/state/hum/thrum.sock", home);
    CHECK(thrum_default_socket_path(got, sizeof got) == 0);
    CHECK_STR(got, want);

    if (saved_home) {
        setenv("HOME", saved_home, 1);
        free(saved_home);
    }
}

typedef struct {
    thrum_client *client;
    int sent;
} sender;

static void *send_many(void *arg) {
    sender *s = arg;
    char *delta = malloc(20001);
    int i;
    memset(delta, 'x', 20000);
    delta[20000] = '\0';
    for (i = 0; i < 100; i++) {
        cJSON *chunk = thrum_tone_new(THRUM_CHI_CHUNK, NULL, "sess-1");
        cJSON_AddStringToObject(chunk, "chunkType", "text_delta");
        cJSON_AddStringToObject(chunk, "delta", delta);
        if (thrum_client_send(s->client, chunk) == 0) s->sent++;
        cJSON_Delete(chunk);
    }
    free(delta);
    return NULL;
}

static void concurrent_sends_never_interleave(void) {
    char sock[600];
    humd_play play = {0};
    pthread_t humd;
    pthread_t threads[8];
    sender senders[8];
    thrum_client *client;
    int i;
    int sent = 0;

    path_in(sock, sizeof sock, "sends.sock");
    play.listen_fd = listen_at(sock);
    pthread_create(&humd, NULL, humd_reads, &play);
    client = thrum_client_new(sock);
    CHECK(thrum_client_connect(client) == 0);
    for (i = 0; i < 8; i++) {
        senders[i].client = client;
        senders[i].sent = 0;
        pthread_create(&threads[i], NULL, send_many, &senders[i]);
    }
    for (i = 0; i < 8; i++) {
        pthread_join(threads[i], NULL);
        sent += senders[i].sent;
    }
    thrum_client_shutdown(client);
    pthread_join(humd, NULL);
    CHECK(sent == 800);
    CHECK(play.lines == 800);
    CHECK(play.bad == 0);
    close(play.listen_fd);
    thrum_client_free(client);
}

static int early_tones;

static void count_early(const cJSON *tone, void *user) {
    (void)user;
    if (str_of(tone, "sid") && strcmp(str_of(tone, "sid"), "early") == 0) early_tones++;
}

static cJSON *a_hello(void) {
    cJSON *hello = cJSON_CreateObject();
    static const char *const roles[] = {"worker"};
    cJSON_AddStringToObject(hello, "hive", "ctest");
    cJSON_AddItemToObject(hello, "roles", cJSON_CreateStringArray(roles, 1));
    cJSON_AddStringToObject(hello, "version", "0.0.1");
    return hello;
}

static void a_signed_hello_is_admitted_on_breath(void) {
    char sock[600];
    humd_play play = {0};
    pthread_t humd;
    thrum_client *client;
    unsigned char seed[32];
    thrum_bee bee;
    cJSON *hello = a_hello();
    cJSON *breath = NULL;

    rfc_seed(seed);
    thrum_bee_from_seed(seed, &bee);
    path_in(sock, sizeof sock, "hello.sock");
    play.listen_fd = listen_at(sock);
    play.script = "admit";
    play.verdict = -1;
    pthread_create(&humd, NULL, humd_hello, &play);

    client = thrum_client_new(sock);
    CHECK(thrum_client_connect(client) == 0);
    thrum_client_on_any(client, count_early, NULL);
    CHECK(thrum_client_hello(client, &bee, hello, &breath) == 0);
    CHECK(breath != NULL);
    CHECK_STR(str_of(breath, "chi"), "breath");
    CHECK(early_tones == 1);
    pthread_join(humd, NULL);
    if (play.verdict != 0) fprintf(stderr, "humd's verdict on the hello: %s\n", play.why);
    CHECK(play.verdict == 0);

    cJSON_Delete(breath);
    cJSON_Delete(hello);
    close(play.listen_fd);
    thrum_client_free(client);
}

typedef struct {
    int hello;
    int run;
    int prompts;
    char error[512];
    char run_error[512];
} hello_outcome;

static void count_prompts(const cJSON *tone, void *user) {
    if (str_of(tone, "chi") && strcmp(str_of(tone, "chi"), "prompt") == 0) {
        ((hello_outcome *)user)->prompts++;
    }
}

static hello_outcome hello_against(const char *script, int timeout_ms) {
    char sock[600];
    char name[64];
    humd_play play = {0};
    pthread_t humd;
    thrum_client *client;
    unsigned char seed[32];
    thrum_bee bee;
    cJSON *hello = a_hello();
    hello_outcome out;

    memset(&out, 0, sizeof out);
    rfc_seed(seed);
    thrum_bee_from_seed(seed, &bee);
    snprintf(name, sizeof name, "%s.sock", script);
    path_in(sock, sizeof sock, name);
    play.listen_fd = listen_at(sock);
    play.script = script;
    pthread_create(&humd, NULL, humd_hello, &play);
    client = thrum_client_new(sock);
    CHECK(thrum_client_connect(client) == 0);
    out.hello = thrum_client_hello_within(client, &bee, hello, NULL, timeout_ms);
    snprintf(out.error, sizeof out.error, "%s", thrum_error());
    if (out.hello == 0) {
        thrum_client_on_any(client, count_prompts, &out);
        out.run = thrum_client_run(client);
        snprintf(out.run_error, sizeof out.run_error, "%s", thrum_error());
    }
    thrum_client_free(client);
    pthread_join(humd, NULL);
    close(play.listen_fd);
    cJSON_Delete(hello);
    return out;
}

static void a_refused_hello_says_why(void) {
    hello_outcome refused = hello_against("refuse", 5000);
    hello_outcome hung_up = hello_against("hang-up", 5000);
    hello_outcome silent = hello_against("silent", 300);

    CHECK(refused.hello != 0);
    CHECK_HAS(refused.error, "hello refused: bee has not entered under this hive");
    CHECK(hung_up.hello != 0);
    CHECK_HAS(hung_up.error, "closed the socket before admitting the hello");
    CHECK(silent.hello != 0);
    CHECK_HAS(silent.error, "no verdict on the hello within 300 ms");
}

static void a_refusal_after_admission_ends_the_run(void) {
    hello_outcome revoked = hello_against("revoke", 5000);
    CHECK(revoked.hello == 0);
    CHECK(revoked.prompts == 1);
    CHECK(revoked.run == -1);
    CHECK_HAS(revoked.run_error, "hello refused: bee identity revoked");
}

typedef struct {
    int a;
    int any;
} tally;

static void on_a(const cJSON *tone, void *user) {
    (void)tone;
    ((tally *)user)->a++;
}

static void on_rest(const cJSON *tone, void *user) {
    (void)tone;
    ((tally *)user)->any++;
}

static void tones_go_to_their_sid_or_the_wildcard(void) {
    char sock[600];
    humd_play play = {0};
    pthread_t humd;
    thrum_client *client;
    tally t = {0, 0};

    path_in(sock, sizeof sock, "dispatch.sock");
    play.listen_fd = listen_at(sock);
    play.text =
        "{\"chi\":\"chunk\",\"rid\":\"1\",\"sid\":\"a\",\"chunkType\":\"text_delta\"}\n"
        "{\"chi\":\"chunk\",\"rid\":\"2\",\"sid\":\"b\",\"chunkType\":\"text_delta\"}\n"
        "{\"chi\":\"pulse\",\"rid\":\"3\"}\n"
        "{\"chi\":\"finish\",\"rid\":\"4\",\"sid\":\"a\",\"finishReason\":\"stop\"}\n";
    pthread_create(&humd, NULL, humd_writes, &play);
    client = thrum_client_new(sock);
    CHECK(thrum_client_connect(client) == 0);
    CHECK(thrum_client_on(client, "a", on_a, &t) == 0);
    CHECK(thrum_client_on(client, "gone", on_a, &t) == 0);
    thrum_client_off(client, "gone");
    thrum_client_on_any(client, on_rest, &t);
    CHECK(thrum_client_run(client) == 0);
    CHECK(t.a == 2);
    CHECK(t.any == 2);
    pthread_join(humd, NULL);
    close(play.listen_fd);
    thrum_client_free(client);
}

static void the_wane_counts_per_sigil(void) {
    thrum_wane *wane = thrum_wane_new();
    CHECK(thrum_wane_get(wane, "s1") == 0);
    CHECK(thrum_wane_tick(wane, "s1") == 1);
    CHECK(thrum_wane_tick(wane, "s1") == 2);
    CHECK(thrum_wane_tick(wane, "s2") == 1);
    CHECK(thrum_wane_behind(wane, "s1", 3));
    CHECK(!thrum_wane_behind(wane, "s1", 2));
    CHECK(thrum_wane_set(wane, "s1", 10) == 0);
    CHECK(thrum_wane_get(wane, "s1") == 10);
    thrum_wane_free(wane);
}

static void dusk_is_past_or_not(void) {
    cJSON *past = cJSON_CreateObject();
    cJSON *future = cJSON_CreateObject();
    cJSON *none = cJSON_CreateObject();
    cJSON_AddNumberToObject(past, "dusk", (double)thrum_dusk_in(-1000));
    cJSON_AddNumberToObject(future, "dusk", (double)thrum_dusk_in(60000));
    CHECK(thrum_is_dusk(past));
    CHECK(!thrum_is_dusk(future));
    CHECK(!thrum_is_dusk(none));
    cJSON_Delete(past);
    cJSON_Delete(future);
    cJSON_Delete(none);
}

int main(int argc, char **argv) {
    const char *golden = argc > 1 ? argv[1] : "../../thrum-core/tests/fixtures/golden.ndjson";
    const char *tmp = getenv("TMPDIR");
    char cleanup[700];

    signal(SIGPIPE, SIG_IGN);
    if (sodium_init() < 0) return 2;
    snprintf(tmp_root, sizeof tmp_root, "%s/thrum-c-XXXXXX", tmp && tmp[0] ? tmp : "/tmp");
    if (!mkdtemp(tmp_root)) {
        fprintf(stderr, "mkdtemp: %s\n", strerror(errno));
        return 2;
    }

    the_humid_encoder_matches_hum_identity();
    a_rid_is_52_crockford_and_garbage_is_not();
    the_sigil_matches_thrum_core();
    the_registry_is_the_generated_one();
    the_golden_fixture_reads_through_the_socket(golden);
    undeclared_keys_are_named_as_humd_names_them();
    a_seed_is_the_bee_hum_identity_derives();
    the_hello_signature_is_the_one_hum_identity_makes();
    a_filed_key_loads_only_as_its_own_id();
    entering_goes_through_the_hum_cli();
    the_socket_resolves_like_hum_paths();
    concurrent_sends_never_interleave();
    a_signed_hello_is_admitted_on_breath();
    a_refused_hello_says_why();
    a_refusal_after_admission_ends_the_run();
    tones_go_to_their_sid_or_the_wildcard();
    the_wane_counts_per_sigil();
    dusk_is_past_or_not();

    snprintf(cleanup, sizeof cleanup, "rm -rf '%s'", tmp_root);
    if (system(cleanup) != 0) fprintf(stderr, "could not remove %s\n", tmp_root);
    printf("%d checks, %d failed\n", checks, failures);
    return failures ? 1 : 0;
}
