/* thrum client: the socket, the line reader, the locked writer, sid
 * dispatch and the signed hello. See client.h.
 */

#include "internal.h"

#include "client.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "chi.h"
#include "helpers.h"

#define READ_CHUNK 65536
#define HELLO_VERDICT_MS 10000
#define TIMED_OUT -2

struct handler {
    char *sid;
    thrum_handler fn;
    void *user;
};

struct thrum_client {
    int fd;
    char path[1024];

    char *rx;
    size_t start;
    size_t scan;
    size_t len;
    size_t cap;
    bool eof;

    pthread_mutex_t send_lock;
    pthread_mutex_t handler_lock;
    struct handler *handlers;
    size_t handler_count;
    thrum_handler any;
    void *any_user;

    char hello_rid[128];
};

thrum_client *thrum_client_new(const char *socket_path) {
    thrum_client *client = (thrum_client *)calloc(1, sizeof *client);
    if (!client) {
        thrum__fail("out of memory");
        return NULL;
    }
    client->fd = -1;
    if (socket_path && socket_path[0]) {
        int n = snprintf(client->path, sizeof client->path, "%s", socket_path);
        if (n < 0 || (size_t)n >= sizeof client->path) {
            free(client);
            thrum__fail("socket path is too long: %s", socket_path);
            return NULL;
        }
    } else if (thrum_default_socket_path(client->path, sizeof client->path) != 0) {
        free(client);
        return NULL;
    }
    pthread_mutex_init(&client->send_lock, NULL);
    pthread_mutex_init(&client->handler_lock, NULL);
    return client;
}

void thrum_client_free(thrum_client *client) {
    size_t i;
    if (!client) return;
    if (client->fd >= 0) close(client->fd);
    free(client->rx);
    for (i = 0; i < client->handler_count; i++) free(client->handlers[i].sid);
    free(client->handlers);
    pthread_mutex_destroy(&client->send_lock);
    pthread_mutex_destroy(&client->handler_lock);
    free(client);
}

const char *thrum_client_socket_path(const thrum_client *client) {
    return client->path;
}

int thrum_client_connect(thrum_client *client) {
    struct sockaddr_un addr;
    size_t len = strlen(client->path);
    int fd;

    if (client->fd >= 0) return 0;
    if (len >= sizeof addr.sun_path) {
        return thrum__fail("socket path is too long for a unix socket: %s", client->path);
    }
    fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return thrum__fail("socket: %s", strerror(errno));
    (void)fcntl(fd, F_SETFD, FD_CLOEXEC);
#ifdef SO_NOSIGPIPE
    {
        int one = 1;
        (void)setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
    }
#endif
    memset(&addr, 0, sizeof addr);
    addr.sun_family = AF_UNIX;
    memcpy(addr.sun_path, client->path, len + 1);
    if (connect(fd, (struct sockaddr *)&addr, sizeof addr) != 0) {
        int err = errno;
        close(fd);
        return thrum__fail("connect %s: %s", client->path, strerror(err));
    }
    client->fd = fd;
    client->eof = false;
    return 0;
}

void thrum_client_shutdown(thrum_client *client) {
    if (client && client->fd >= 0) (void)shutdown(client->fd, SHUT_RDWR);
}

static int write_all(int fd, const char *bytes, size_t n) {
    while (n > 0) {
#ifdef MSG_NOSIGNAL
        ssize_t w = send(fd, bytes, n, MSG_NOSIGNAL);
#else
        ssize_t w = write(fd, bytes, n);
#endif
        if (w < 0) {
            if (errno == EINTR) continue;
            return thrum__fail("write: %s", strerror(errno));
        }
        bytes += w;
        n -= (size_t)w;
    }
    return 0;
}

int thrum_client_send(thrum_client *client, const cJSON *tone) {
    char *json;
    char *line;
    size_t n;
    int rc;

    if (client->fd < 0) return thrum__fail("not connected");
    if (!cJSON_IsObject(tone)) return thrum__fail("a tone is a JSON object");
    json = cJSON_PrintUnformatted(tone);
    if (!json) return thrum__fail("could not serialise the tone");
    n = strlen(json);
    line = (char *)malloc(n + 1);
    if (!line) {
        cJSON_free(json);
        return thrum__fail("out of memory");
    }
    memcpy(line, json, n);
    line[n] = '\n';
    cJSON_free(json);

    pthread_mutex_lock(&client->send_lock);
    rc = write_all(client->fd, line, n + 1);
    pthread_mutex_unlock(&client->send_lock);
    free(line);
    return rc;
}

static int fill(thrum_client *client) {
    ssize_t n;

    if (client->start > 0) {
        memmove(client->rx, client->rx + client->start, client->len - client->start);
        client->len -= client->start;
        client->scan -= client->start;
        client->start = 0;
    }
    if (client->cap - client->len < READ_CHUNK + 1) {
        size_t cap = client->cap ? client->cap : READ_CHUNK * 2;
        char *grown;
        while (cap - client->len < READ_CHUNK + 1) cap *= 2;
        grown = (char *)realloc(client->rx, cap);
        if (!grown) return thrum__fail("out of memory");
        client->rx = grown;
        client->cap = cap;
    }
    do {
        n = read(client->fd, client->rx + client->len, client->cap - client->len - 1);
    } while (n < 0 && errno == EINTR);
    if (n < 0) return thrum__fail("read: %s", strerror(errno));
    client->len += (size_t)n;
    return n > 0 ? 1 : 0;
}

static cJSON *parse_line(char *line, size_t n) {
    cJSON *value;
    while (n > 0 && (line[n - 1] == '\r' || line[n - 1] == ' ' || line[n - 1] == '\t')) {
        line[--n] = '\0';
    }
    if (n == 0) return NULL;
    value = cJSON_ParseWithLengthOpts(line, n + 1, NULL, 1);
    if (!cJSON_IsObject(value)) {
        cJSON_Delete(value);
        return NULL;
    }
    return value;
}

static int readable_by(thrum_client *client, int64_t deadline) {
    for (;;) {
        struct pollfd pfd;
        int64_t left = deadline - thrum_now_ms();
        int rc;
        if (left <= 0) return TIMED_OUT;
        pfd.fd = client->fd;
        pfd.events = POLLIN;
        pfd.revents = 0;
        rc = poll(&pfd, 1, left > 60000 ? 60000 : (int)left);
        if (rc > 0) return 0;
        if (rc < 0 && errno != EINTR) return thrum__fail("poll: %s", strerror(errno));
    }
}

static int next_tone(thrum_client *client, cJSON **tone, int64_t deadline) {
    *tone = NULL;
    if (client->fd < 0) return thrum__fail("not connected");
    for (;;) {
        char *nl = client->len > client->scan
                       ? (char *)memchr(client->rx + client->scan, '\n', client->len - client->scan)
                       : NULL;
        if (nl) {
            size_t begin = client->start;
            size_t end = (size_t)(nl - client->rx);
            client->start = client->scan = end + 1;
            *nl = '\0';
            *tone = parse_line(client->rx + begin, end - begin);
            if (*tone) return 1;
            continue;
        }
        client->scan = client->len;
        if (client->eof) {
            if (client->len > client->start) {
                size_t begin = client->start;
                client->rx[client->len] = '\0';
                client->start = client->scan = client->len;
                *tone = parse_line(client->rx + begin, client->len - begin);
                if (*tone) return 1;
            }
            return 0;
        }
        if (deadline > 0) {
            int ready = readable_by(client, deadline);
            if (ready != 0) return ready;
        }
        switch (fill(client)) {
            case -1: return -1;
            case 0: client->eof = true; break;
            default: break;
        }
    }
}

int thrum_client_recv(thrum_client *client, cJSON **tone) {
    return next_tone(client, tone, 0);
}

int thrum_client_on(thrum_client *client, const char *sid, thrum_handler fn, void *user) {
    size_t i;
    int rc = 0;
    pthread_mutex_lock(&client->handler_lock);
    for (i = 0; i < client->handler_count; i++) {
        if (strcmp(client->handlers[i].sid, sid) == 0) {
            client->handlers[i].fn = fn;
            client->handlers[i].user = user;
            pthread_mutex_unlock(&client->handler_lock);
            return 0;
        }
    }
    {
        struct handler *grown = (struct handler *)realloc(
            client->handlers, (client->handler_count + 1) * sizeof *grown);
        char *copy = strdup(sid);
        if (!grown || !copy) {
            if (grown) client->handlers = grown;
            free(copy);
            rc = thrum__fail("out of memory");
        } else {
            client->handlers = grown;
            client->handlers[client->handler_count].sid = copy;
            client->handlers[client->handler_count].fn = fn;
            client->handlers[client->handler_count].user = user;
            client->handler_count++;
        }
    }
    pthread_mutex_unlock(&client->handler_lock);
    return rc;
}

void thrum_client_off(thrum_client *client, const char *sid) {
    size_t i;
    pthread_mutex_lock(&client->handler_lock);
    for (i = 0; i < client->handler_count; i++) {
        if (strcmp(client->handlers[i].sid, sid) == 0) {
            free(client->handlers[i].sid);
            memmove(&client->handlers[i], &client->handlers[i + 1],
                    (client->handler_count - i - 1) * sizeof client->handlers[0]);
            client->handler_count--;
            break;
        }
    }
    pthread_mutex_unlock(&client->handler_lock);
}

void thrum_client_on_any(thrum_client *client, thrum_handler fn, void *user) {
    pthread_mutex_lock(&client->handler_lock);
    client->any = fn;
    client->any_user = user;
    pthread_mutex_unlock(&client->handler_lock);
}

static void dispatch(thrum_client *client, const cJSON *tone) {
    const cJSON *sid = cJSON_GetObjectItemCaseSensitive(tone, "sid");
    thrum_handler fn = NULL;
    void *user = NULL;
    size_t i;

    pthread_mutex_lock(&client->handler_lock);
    if (cJSON_IsString(sid)) {
        for (i = 0; i < client->handler_count; i++) {
            if (strcmp(client->handlers[i].sid, sid->valuestring) == 0) {
                fn = client->handlers[i].fn;
                user = client->handlers[i].user;
                break;
            }
        }
    }
    if (!fn) {
        fn = client->any;
        user = client->any_user;
    }
    pthread_mutex_unlock(&client->handler_lock);
    if (fn) fn(tone, user);
}

static const char *string_of(const cJSON *tone, const char *key) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(tone, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static bool is_chi(const cJSON *tone, const char *chi) {
    const char *value = string_of(tone, "chi");
    return value && strcmp(value, chi) == 0;
}

static bool answers_hello(const thrum_client *client, const cJSON *tone) {
    const char *rid = string_of(tone, "rid");
    return client->hello_rid[0] && rid && strcmp(rid, client->hello_rid) == 0;
}

static bool refuses_hello(const thrum_client *client, const cJSON *tone) {
    const char *why;
    if (!is_chi(tone, THRUM_CHI_ECHO) || !answers_hello(client, tone)) return false;
    if (!cJSON_IsFalse(cJSON_GetObjectItemCaseSensitive(tone, "ok"))) return false;
    why = string_of(tone, "error");
    thrum__fail("hello refused: %s", why ? why : "no reason given");
    return true;
}

int thrum_client_run(thrum_client *client) {
    for (;;) {
        cJSON *tone = NULL;
        int got = thrum_client_recv(client, &tone);
        if (got <= 0) return got;
        if (refuses_hello(client, tone)) {
            cJSON_Delete(tone);
            return -1;
        }
        dispatch(client, tone);
        cJSON_Delete(tone);
    }
}

static int prepare_hello(thrum_client *client, const thrum_bee *bee, cJSON *hello) {
    const char *rid;
    if (!cJSON_IsObject(hello)) return thrum__fail("a hello is a JSON object");
    if (thrum__set_string(hello, "chi", THRUM_CHI_HELLO) != 0) return -1;
    if (!cJSON_IsString(cJSON_GetObjectItemCaseSensitive(hello, "rid"))) {
        char minted[THRUM_RID_LEN];
        thrum_rid(minted);
        if (thrum__set_string(hello, "rid", minted) != 0) return -1;
    }
    rid = string_of(hello, "rid");
    if (strlen(rid) >= sizeof client->hello_rid) return thrum__fail("the hello's rid is too long");
    if (!cJSON_IsString(cJSON_GetObjectItemCaseSensitive(hello, "protoVersion"))) {
        if (thrum__set_string(hello, "protoVersion", THRUM_VERSION) != 0) return -1;
    }
    if (thrum_bee_sign_hello(bee, hello) != 0) return -1;
    memcpy(client->hello_rid, rid, strlen(rid) + 1);
    return 0;
}

int thrum_client_hello_within(thrum_client *client, const thrum_bee *bee, cJSON *hello,
                              cJSON **breath, int timeout_ms) {
    int64_t deadline;
    bool delivered = false;

    if (breath) *breath = NULL;
    if (timeout_ms <= 0) return thrum__fail("a hello needs a positive timeout");
    if (prepare_hello(client, bee, hello) != 0) return -1;
    if (thrum_client_send(client, hello) != 0) return -1;
    deadline = thrum_now_ms() + timeout_ms;

    for (;;) {
        cJSON *tone = NULL;
        int got = next_tone(client, &tone, deadline);
        if (got == TIMED_OUT) {
            return thrum__fail("humd gave no verdict on the hello within %d ms", timeout_ms);
        }
        if (got < 0) return -1;
        if (got == 0) return thrum__fail("humd closed the socket before admitting the hello");
        if (refuses_hello(client, tone)) {
            cJSON_Delete(tone);
            return -1;
        }
        if (is_chi(tone, THRUM_CHI_ERROR)) {
            const char *why = string_of(tone, "message");
            thrum__fail("hello refused: %s", why ? why : "no reason given");
            cJSON_Delete(tone);
            return -1;
        }
        if (is_chi(tone, THRUM_CHI_ECHO) && answers_hello(client, tone)) {
            delivered = true;
            cJSON_Delete(tone);
            continue;
        }
        if (delivered && is_chi(tone, THRUM_CHI_BREATH)) {
            if (breath) {
                *breath = tone;
            } else {
                cJSON_Delete(tone);
            }
            return 0;
        }
        dispatch(client, tone);
        cJSON_Delete(tone);
    }
}

int thrum_client_hello(thrum_client *client, const thrum_bee *bee, cJSON *hello, cJSON **breath) {
    return thrum_client_hello_within(client, bee, hello, breath, HELLO_VERDICT_MS);
}

cJSON *thrum_tone_new(const char *chi, const char *rid, const char *sid) {
    char minted[THRUM_RID_LEN];
    cJSON *tone = cJSON_CreateObject();

    if (!tone) {
        thrum__fail("out of memory");
        return NULL;
    }
    if (!rid) {
        thrum_rid(minted);
        rid = minted;
    }
    if (!cJSON_AddStringToObject(tone, "chi", chi) || !cJSON_AddStringToObject(tone, "rid", rid) ||
        (sid && !cJSON_AddStringToObject(tone, "sid", sid))) {
        cJSON_Delete(tone);
        thrum__fail("out of memory");
        return NULL;
    }
    return tone;
}
