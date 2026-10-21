/* thrum client — the C reference client. See client.h. */

#include "client.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

void thrum_client_init(thrum_client *client, const char *socket_path) {
    memset(client, 0, sizeof *client);
    client->fd = -1;
    if (socket_path && socket_path[0]) {
        snprintf(client->socket_path, sizeof client->socket_path, "%s", socket_path);
    } else {
        thrum_default_socket_path(client->socket_path, sizeof client->socket_path);
    }
}

void thrum_client_free(thrum_client *client) {
    size_t i;
    thrum_client_close(client);
    for (i = 0; i < client->handler_count; i++) free(client->handlers[i].sid);
    free(client->handlers);
    client->handlers = NULL;
    client->handler_count = 0;
}

int thrum_client_connect(thrum_client *client) {
    struct sockaddr_un addr;
    int fd;

    if (client->fd >= 0) return 0;

    if (strlen(client->socket_path) >= sizeof addr.sun_path) {
        fprintf(stderr, "thrum: socket path too long: %s\n", client->socket_path);
        return -1;
    }
    fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        fprintf(stderr, "thrum: socket: %s\n", strerror(errno));
        return -1;
    }
    memset(&addr, 0, sizeof addr);
    addr.sun_family = AF_UNIX;
    snprintf(addr.sun_path, sizeof addr.sun_path, "%s", client->socket_path);
    if (connect(fd, (struct sockaddr *)&addr, sizeof addr) < 0) {
        fprintf(stderr, "thrum: connect %s: %s\n", client->socket_path, strerror(errno));
        close(fd);
        return -1;
    }
    client->fd = fd;
    return 0;
}

void thrum_client_close(thrum_client *client) {
    if (client->fd >= 0) close(client->fd);
    client->fd = -1;
    free(client->rx);
    client->rx = NULL;
    client->rx_len = 0;
    client->rx_cap = 0;
}

int thrum_client_send_line(thrum_client *client, const char *line) {
    size_t len = strlen(line);
    size_t off = 0;
    while (off < len) {
        ssize_t n = write(client->fd, line + off, len - off);
        if (n < 0) {
            if (errno == EINTR) continue;
            fprintf(stderr, "thrum: write: %s\n", strerror(errno));
            return -1;
        }
        off += (size_t)n;
    }
    for (;;) {
        const char nl = '\n';
        ssize_t n = write(client->fd, &nl, 1);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) {
            fprintf(stderr, "thrum: write: %s\n", strerror(errno));
            return -1;
        }
        break;
    }
    return 0;
}

int thrum_client_send(thrum_client *client, const thrum_buf *tone) {
    if (!tone || !tone->data) return 0;
    return thrum_client_send_line(client, tone->data);
}

void thrum_client_on(thrum_client *client, const char *sid, thrum_handler fn, void *user) {
    size_t i;
    for (i = 0; i < client->handler_count; i++) {
        if (strcmp(client->handlers[i].sid, sid) == 0) {
            client->handlers[i].fn = fn;
            client->handlers[i].user = user;
            return;
        }
    }
    client->handlers = (thrum_handler_entry *)realloc(
        client->handlers, (client->handler_count + 1) * sizeof(thrum_handler_entry));
    if (!client->handlers) {
        fprintf(stderr, "thrum: out of memory\n");
        exit(70);
    }
    client->handlers[client->handler_count].sid = strdup(sid);
    client->handlers[client->handler_count].fn = fn;
    client->handlers[client->handler_count].user = user;
    client->handler_count++;
}

void thrum_client_off(thrum_client *client, const char *sid) {
    size_t i;
    for (i = 0; i < client->handler_count; i++) {
        if (strcmp(client->handlers[i].sid, sid) == 0) {
            free(client->handlers[i].sid);
            memmove(&client->handlers[i], &client->handlers[i + 1],
                    (client->handler_count - i - 1) * sizeof(thrum_handler_entry));
            client->handler_count--;
            return;
        }
    }
}

void thrum_client_on_any(thrum_client *client, thrum_handler fn, void *user) {
    client->any = fn;
    client->any_user = user;
}

static ssize_t take_line(thrum_client *client, char **line) {
    char *nl = (char *)memchr(client->rx, '\n', client->rx_len);
    if (!nl) return 0;
    *line = client->rx;
    {
        size_t len = (size_t)(nl - client->rx);
        memmove(client->rx, nl + 1, client->rx_len - len - 1);
        client->rx_len -= len + 1;
        *nl = '\0';
        return (ssize_t)len;
    }
}

static int fill(thrum_client *client) {
    char chunk[4096];
    ssize_t n;
    do {
        n = read(client->fd, chunk, sizeof chunk);
    } while (n < 0 && errno == EINTR);
    if (n < 0) {
        fprintf(stderr, "thrum: read: %s\n", strerror(errno));
        return -1;
    }
    if (n == 0) return 0;
    if (client->rx_len + (size_t)n + 1 > client->rx_cap) {
        size_t cap = client->rx_cap ? client->rx_cap : 4096;
        while (cap < client->rx_len + (size_t)n + 1) cap *= 2;
        client->rx = (char *)realloc(client->rx, cap);
        if (!client->rx) {
            fprintf(stderr, "thrum: out of memory\n");
            exit(70);
        }
        client->rx_cap = cap;
    }
    memcpy(client->rx + client->rx_len, chunk, (size_t)n);
    client->rx_len += (size_t)n;
    client->rx[client->rx_len] = '\0';
    return 1;
}

int thrum_client_recv(thrum_client *client, thrum_json **out) {
    for (;;) {
        char *line = NULL;
        ssize_t len = take_line(client, &line);
        if (len < 0) return 0;
        if (len == 0) {
            int rc = fill(client);
            if (rc < 0) return -1;
            if (rc == 0) {
                
                if (client->rx_len == 0) return 0;
                line = client->rx;
                len = (ssize_t)client->rx_len;
                client->rx_len = 0;
            } else {
                continue;
            }
        }
        if (len == 0) continue;
        {
            thrum_json *value = thrum_json_parse(line, (size_t)len);
            if (!value) continue; 
            *out = value;
            return 1;
        }
    }
}

static void dispatch(thrum_client *client, const thrum_json *tone) {
    const char *sid = thrum_json_str(thrum_json_get(tone, "sid"), NULL);
    thrum_handler fn = NULL;
    void *user = NULL;
    size_t i;
    if (sid) {
        for (i = 0; i < client->handler_count; i++) {
            if (strcmp(client->handlers[i].sid, sid) == 0) {
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
    if (fn) fn(tone, user);
}

int thrum_client_run(thrum_client *client) {
    for (;;) {
        thrum_json *tone = NULL;
        int got = thrum_client_recv(client, &tone);
        if (got < 0) return -1;
        if (got == 0) return 0;
        dispatch(client, tone);
        thrum_json_free(tone);
    }
}

int thrum_client_hello(thrum_client *client, const char *bee, const char *version,
                       const char *chis_json, const char *propensity_json) {
    thrum_buf tone;
    int rc;

    thrum_buf_init(&tone);
    thrum_tone_begin(&tone, CHI_HELLO, "hello-1", NULL);
    thrum_tone_str(&tone, "bee", bee);
    thrum_tone_str(&tone, "version", version);
    thrum_tone_str(&tone, "protoVersion", THRUM_VERSION);
    if (propensity_json) thrum_tone_raw(&tone, "propensity", propensity_json);
    if (chis_json) thrum_tone_raw(&tone, "chis", chis_json);
    thrum_tone_end(&tone);

    rc = thrum_client_send(client, &tone);
    thrum_buf_free(&tone);
    if (rc != 0) return rc;

    for (;;) {
        thrum_json *in = NULL;
        int got = thrum_client_recv(client, &in);
        if (got <= 0) return -1;
        {
            const char *chi = thrum_json_str(thrum_json_get(in, "chi"), "");
            if (strcmp(chi, CHI_BREATH) == 0) {
                thrum_json_free(in);
                return 0;
            }
            if (strcmp(chi, CHI_ERROR) == 0) {
                fprintf(stderr, "thrum: hello refused: %s\n",
                        thrum_json_str(thrum_json_get(in, "message"), "unknown"));
                thrum_json_free(in);
                return -1;
            }
        }
        thrum_json_free(in);
    }
}

void thrum_tone_begin(thrum_buf *buf, const char *chi, const char *rid, const char *sid) {
    thrum_buf_append_str(buf, "{\"chi\":");
    thrum_buf_append_json_string(buf, chi);
    if (rid) {
        thrum_buf_append_str(buf, ",\"rid\":");
        thrum_buf_append_json_string(buf, rid);
    }
    if (sid) {
        thrum_buf_append_str(buf, ",\"sid\":");
        thrum_buf_append_json_string(buf, sid);
    }
}

void thrum_tone_str(thrum_buf *buf, const char *key, const char *value) {
    if (!value) return;
    thrum_buf_append_str(buf, ",");
    thrum_buf_append_json_string(buf, key);
    thrum_buf_append_str(buf, ":");
    thrum_buf_append_json_string(buf, value);
}

void thrum_tone_strn(thrum_buf *buf, const char *key, const char *value, size_t len) {
    if (!value) return;
    thrum_buf_append_str(buf, ",");
    thrum_buf_append_json_string(buf, key);
    thrum_buf_append_str(buf, ":");
    thrum_buf_append_json_string_n(buf, value, len);
}

void thrum_tone_int(thrum_buf *buf, const char *key, int64_t value) {
    char num[32];
    snprintf(num, sizeof num, "%lld", (long long)value);
    thrum_buf_append_str(buf, ",");
    thrum_buf_append_json_string(buf, key);
    thrum_buf_append_str(buf, ":");
    thrum_buf_append_str(buf, num);
}

void thrum_tone_raw(thrum_buf *buf, const char *key, const char *json) {
    if (!json) return;
    thrum_buf_append_str(buf, ",");
    thrum_buf_append_json_string(buf, key);
    thrum_buf_append_str(buf, ":");
    thrum_buf_append_json_raw(buf, json);
}

void thrum_tone_end(thrum_buf *buf) {
    thrum_buf_append_str(buf, "}");
}
