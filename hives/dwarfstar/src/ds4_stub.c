#define _POSIX_C_SOURCE 200809L

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "ds4_bridge.h"
#include "shaders.h"

#define STUB_LOAD_MS 300
#define STUB_PIECE_MS 50

#define USER_TURN "<｜User｜>"
#define MODEL_TURN "<｜Assistant｜></think>"
#define TOOLS_OFFERED "### Available Tool Schemas\n\n"

struct ds4_bridge {
    char path[1024];
    atomic_int cancelled;
};

static void sleep_ms(int ms) {
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

ds4_bridge *ds4_bridge_new(const ds4_bridge_options *options) {
    ds4_bridge *bridge = (ds4_bridge *)calloc(1, sizeof *bridge);
    (void)options;
    if (bridge) atomic_init(&bridge->cancelled, 0);
    return bridge;
}

void ds4_bridge_free(ds4_bridge *bridge) {
    free(bridge);
}

bool ds4_bridge_loaded(const ds4_bridge *bridge) {
    return bridge->path[0] != '\0';
}

bool ds4_bridge_ready(const ds4_bridge *bridge, const char *model_path) {
    return bridge->path[0] && strcmp(bridge->path, model_path) == 0;
}

int ds4_bridge_load(ds4_bridge *bridge, const char *model_path, char *err, size_t errlen) {
    if (strlen(model_path) >= sizeof bridge->path) {
        snprintf(err, errlen, "model path is too long");
        return -1;
    }
    sleep_ms(STUB_LOAD_MS);
    snprintf(bridge->path, sizeof bridge->path, "%s", model_path);
    return 0;
}

void ds4_bridge_unload(ds4_bridge *bridge) {
    bridge->path[0] = '\0';
}

dialect ds4_bridge_dialect(const ds4_bridge *bridge) {
    (void)bridge;
    return DIALECT_DEEPSEEK;
}

void ds4_bridge_arm(ds4_bridge *bridge) {
    atomic_store(&bridge->cancelled, 0);
}

void ds4_bridge_cancel(ds4_bridge *bridge) {
    if (bridge) atomic_store(&bridge->cancelled, 1);
}

bool ds4_bridge_cancelled(ds4_bridge *bridge) {
    return atomic_load(&bridge->cancelled) != 0;
}

static bool begin(ds4_bridge *bridge, ds4_wilt *wilt, char *err, size_t errlen) {
    wilt->tokens_in = 0;
    wilt->tokens_out = 0;
    wilt->reason = "stop";
    err[0] = '\0';
    if (bridge->path[0]) return true;
    snprintf(err, errlen, "no model is loaded");
    return false;
}

static bool say_piece(ds4_bridge *bridge, const char *piece, size_t len, int max_tokens,
                      ds4_piece_fn on_piece, void *user, ds4_wilt *wilt) {
    if (atomic_load(&bridge->cancelled)) {
        wilt->reason = "cancel";
        return false;
    }
    if (wilt->tokens_out >= max_tokens) {
        wilt->reason = "max_tokens";
        return false;
    }
    on_piece(user, piece, len);
    wilt->tokens_out++;
    sleep_ms(STUB_PIECE_MS);
    return true;
}

static bool say_word_by_word(ds4_bridge *bridge, const char *words, int max_tokens,
                             ds4_piece_fn on_piece, void *user, ds4_wilt *wilt) {
    const char *at = words;
    while (*at) {
        size_t n = strcspn(at, " ");
        size_t step = n + (at[n] == ' ' ? 1 : 0);
        if (!say_piece(bridge, at, step, max_tokens, on_piece, user, wilt)) return false;
        at += step;
    }
    return true;
}

static bool say_e_acute_split_in_two(ds4_bridge *bridge, int max_tokens, ds4_piece_fn on_piece,
                                     void *user, ds4_wilt *wilt) {
    return say_piece(bridge, " \xC3", 2, max_tokens, on_piece, user, wilt) &&
           say_piece(bridge, "\xA9", 1, max_tokens, on_piece, user, wilt);
}

int ds4_bridge_generate(ds4_bridge *bridge, const char *system, const ds4_message *messages,
                        size_t count, float temperature, int max_tokens, ds4_piece_fn on_piece,
                        void *user, ds4_wilt *wilt, char *err, size_t errlen) {
    (void)system;
    (void)temperature;
    if (!begin(bridge, wilt, err, errlen)) return -1;
    if (count == 0) {
        snprintf(err, errlen, "the prompt carried no text");
        return -1;
    }
    wilt->tokens_in = (int)strlen(messages[count - 1].text);
    if (say_word_by_word(bridge, messages[count - 1].text, max_tokens, on_piece, user, wilt)) {
        say_e_acute_split_in_two(bridge, max_tokens, on_piece, user, wilt);
    }
    return 0;
}

static char *between(const char *text, const char *after, const char *before) {
    const char *start = text ? strstr(text, after) : NULL;
    const char *end;
    if (!start) return NULL;
    start += strlen(after);
    end = strstr(start, before);
    if (!end) end = start + strlen(start);
    {
        char *copy = (char *)malloc((size_t)(end - start) + 1);
        if (!copy) return NULL;
        memcpy(copy, start, (size_t)(end - start));
        copy[end - start] = '\0';
        return copy;
    }
}

static const char *last_of(const char *text, const char *needle) {
    const char *last = NULL;
    const char *at = text;
    while ((at = strstr(at, needle)) != NULL) last = at++;
    return last;
}

static bool ends_with(const char *text, const char *suffix) {
    size_t n = strlen(text);
    size_t k = strlen(suffix);
    return n >= k && strcmp(text + n - k, suffix) == 0;
}

static bool tell_what_the_tool_said(ds4_bridge *bridge, const char *prompt, int max_tokens,
                                    ds4_piece_fn on_piece, void *user, ds4_wilt *wilt) {
    char *answer = between(last_of(prompt, "<tool_result>"), "<tool_result>", "</tool_result>");
    bool said = say_word_by_word(bridge, "the tool said ", max_tokens, on_piece, user, wilt) &&
                say_word_by_word(bridge, answer ? answer : "", max_tokens, on_piece, user, wilt);
    free(answer);
    return said;
}

static char *tool_named_in(const char *prompt, const char *asked) {
    const char *offered = strstr(prompt, TOOLS_OFFERED);
    char *first = between(offered, "\"name\":\"", "\"");
    const char *at = offered;
    while (at && asked && (at = strstr(at, "\n{\"name\":\"")) != NULL) {
        char *name = between(at, "\"name\":\"", "\"");
        at++;
        if (name && strstr(asked, name)) {
            free(first);
            return name;
        }
        free(name);
    }
    return first;
}

static bool reach_for_the_tool_asked_for(ds4_bridge *bridge, const char *prompt, int max_tokens,
                                         ds4_piece_fn on_piece, void *user, ds4_wilt *wilt) {
    char *asked = between(last_of(prompt, USER_TURN), USER_TURN, MODEL_TURN);
    char *name = tool_named_in(prompt, asked);
    char invoke[512];
    char parameter[2048];
    bool said;

    snprintf(invoke, sizeof invoke, "_calls>\n<｜DSML｜invoke name=\"%s\">\n", name ? name : "");
    snprintf(parameter, sizeof parameter,
             "<｜DSML｜parameter name=\"asked\" string=\"true\">%s</｜DSML｜parameter>\n"
             "</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
             asked ? asked : "");
    said = say_word_by_word(bridge, "let me ask", max_tokens, on_piece, user, wilt) &&
           say_piece(bridge, "\n\n<｜DSML｜tool", strlen("\n\n<｜DSML｜tool"), max_tokens, on_piece,
                     user, wilt) &&
           say_piece(bridge, invoke, strlen(invoke), max_tokens, on_piece, user, wilt) &&
           say_piece(bridge, parameter, strlen(parameter), max_tokens, on_piece, user, wilt);
    free(name);
    free(asked);
    return said;
}

int ds4_bridge_generate_from_prompt(ds4_bridge *bridge, const char *prompt, float temperature,
                                    int max_tokens, ds4_piece_fn on_piece, void *user,
                                    ds4_wilt *wilt, char *err, size_t errlen) {
    char *asked;
    (void)temperature;
    if (!begin(bridge, wilt, err, errlen)) return -1;
    wilt->tokens_in = (int)strlen(prompt);
    if (ends_with(prompt, "</tool_result>" MODEL_TURN)) {
        tell_what_the_tool_said(bridge, prompt, max_tokens, on_piece, user, wilt);
        return 0;
    }
    if (strstr(prompt, TOOLS_OFFERED)) {
        reach_for_the_tool_asked_for(bridge, prompt, max_tokens, on_piece, user, wilt);
        return 0;
    }
    asked = between(last_of(prompt, USER_TURN), USER_TURN, MODEL_TURN);
    say_word_by_word(bridge, asked ? asked : "", max_tokens, on_piece, user, wilt);
    free(asked);
    return 0;
}

int dwarfstar_shaders_point_at(const char *dir) {
    (void)dir;
    return 0;
}
