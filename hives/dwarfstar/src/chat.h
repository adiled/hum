#ifndef DWARFSTAR_CHAT_H
#define DWARFSTAR_CHAT_H

#include <stdbool.h>
#include <stddef.h>

#include <cjson/cJSON.h>

typedef enum {
    DIALECT_DEEPSEEK,
    DIALECT_DEEPSEEK41,
    DIALECT_GLM,
    DIALECT_QWEN,
} dialect;

typedef enum {
    USER_SAYS,
    MODEL_SAYS,
    TOOL_ANSWERS,
} speaker;

typedef struct {
    char *call_id;
    char *name;
    char *args_json;
} tendril;

typedef struct {
    speaker by;
    char *words;
    char *answering_call_id;
    tendril *tendrils;
    size_t tendril_count;
    char *tendrils_as_sampled;
} turn;

typedef struct {
    turn *turns;
    size_t count;
    size_t room;
} chat;

typedef struct {
    size_t words_end;
    size_t tendrils_start;
    size_t tendrils_end;
    tendril *tendrils;
    size_t count;
} reaching;

turn *chat_add_turn(chat *c, speaker by);
turn *chat_last_turn(const chat *c);
int turn_add_words(turn *t, const char *words, const char *joiner);
int turn_add_tendril(turn *t, const char *call_id, const char *name, const char *args_json);
void chat_free(chat *c);

char *chat_as_prompt(dialect d, const char *system, const cJSON *tools, const chat *c);

size_t words_safe_to_show(const char *grown, bool *tendrils_began);

bool read_tendrils(dialect d, const char *grown, const cJSON *tools, reaching *out);
void reaching_free(reaching *r);

#endif
