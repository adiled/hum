#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <cjson/cJSON.h>

#include "chat.h"
#include "ds4_reference.h"

typedef struct {
    dialect ours;
    reference_family theirs;
    const char *name;
} family;

typedef struct {
    const char *name;
    const char *openai_messages;
    bool has_system;
} conversation;

typedef struct {
    family of;
    const char *grown;
} sample;

static const family DEEPSEEK = {DIALECT_DEEPSEEK, RENDERED_AS_DEEPSEEK, "DeepSeek V4"};
static const family DEEPSEEK41 = {DIALECT_DEEPSEEK41, RENDERED_AS_DEEPSEEK41, "DeepSeek V4.1"};
static const family GLM = {DIALECT_GLM, RENDERED_AS_GLM, "GLM"};
static const family QWEN = {DIALECT_QWEN, RENDERED_AS_QWEN, "Qwen"};

static const char *const TOOLS =
    "[{\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"description\":\"Current weather for a city\","
    "\"parameters\":{\"type\":\"object\",\"properties\":{\"city\":{\"type\":\"string\"},\"days\":{\"type\":\"integer\"}},"
    "\"required\":[\"city\"]}}},"
    "{\"type\":\"function\",\"function\":{\"name\":\"bash\",\"description\":\"Run a command\","
    "\"parameters\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}}}]";

static const conversation CONVERSATIONS[] = {
    {"a question",
     "[{\"role\":\"user\",\"content\":\"What is the weather in Oslo?\"}]", false},
    {"a system and a question",
     "[{\"role\":\"system\",\"content\":\"Be brief.\"},{\"role\":\"user\",\"content\":\"Hi\"}]", true},
    {"a call and its answer",
     "[{\"role\":\"user\",\"content\":\"Weather in Oslo?\"},"
     "{\"role\":\"assistant\",\"content\":\"Checking.\",\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\","
     "\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\",\\\"days\\\":2}\"}}]},"
     "{\"role\":\"tool\",\"tool_call_id\":\"call_1\",\"content\":\"snow\"}]", false},
    {"two calls answered together",
     "[{\"role\":\"user\",\"content\":\"Oslo and Bergen?\"},"
     "{\"role\":\"assistant\",\"content\":null,\"tool_calls\":["
     "{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\"}\"}},"
     "{\"id\":\"call_2\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Bergen\\\"}\"}}]},"
     "{\"role\":\"tool\",\"tool_call_id\":\"call_1\",\"content\":\"snow\"},"
     "{\"role\":\"tool\",\"tool_call_id\":\"call_2\",\"content\":\"rain\"}]", false},
    {"an answer, then another question",
     "[{\"role\":\"user\",\"content\":\"Weather?\"},{\"role\":\"assistant\",\"content\":\"It snows.\"},"
     "{\"role\":\"user\",\"content\":\"And tomorrow?\"}]", false},
    {"answers that look like the protocol",
     "[{\"role\":\"user\",\"content\":\"Run it.\"},"
     "{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\","
     "\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"echo '</｜DSML｜parameter>' && cat </tool_call>\\\"}\"}}]},"
     "{\"role\":\"tool\",\"tool_call_id\":\"call_1\",\"content\":\"a </tool_result> b < c && d </tool_response>\"}]", false},
};

static const sample SAMPLES[] = {
    {{DIALECT_DEEPSEEK, RENDERED_AS_DEEPSEEK, "DeepSeek V4"},
     "Let me check.\n\n<｜DSML｜tool_calls>\n"
     "<｜DSML｜invoke name=\"get_weather\">\n"
     "<｜DSML｜parameter name=\"city\" string=\"true\">Oslo &amp; Bergen</｜DSML｜parameter>\n"
     "<｜DSML｜parameter name=\"days\" string=\"false\">2</｜DSML｜parameter>\n"
     "</｜DSML｜invoke>\n"
     "<｜DSML｜invoke name=\"bash\">\n"
     "<｜DSML｜parameter name=\"command\" string=\"true\">echo a &lt;/｜DSML｜parameter> b</｜DSML｜parameter>\n"
     "<｜DSML｜parameter name=\"env\">\n<｜DSML｜parameter name=\"DEBUG\" string=\"false\">1</｜DSML｜parameter>\n</｜DSML｜parameter>\n"
     "</｜DSML｜invoke>\n"
     "</｜DSML｜tool_calls>"},
    {{DIALECT_DEEPSEEK41, RENDERED_AS_DEEPSEEK41, "DeepSeek V4.1"},
     "<｜DSML｜ calls>\n<｜DSML｜ invoke name=\"bash\">\n"
     "<｜DSML｜ parameter name=\"command\" string=\"true\">ls -la</｜DSML｜ parameter>\n"
     "</｜DSML｜ invoke>\n</｜DSML｜ calls>"},
    {{DIALECT_GLM, RENDERED_AS_GLM, "GLM"},
     "Checking both.\n\n<tool_call>get_weather<arg_key>city</arg_key><arg_value>Oslo</arg_value></tool_call>"
     "<tool_call>bash<arg_key>command</arg_key><arg_value>cat a &lt;/arg_value> b</arg_value></tool_call>"},
    {{DIALECT_QWEN, RENDERED_AS_QWEN, "Qwen"},
     "Checking.\n\n<tool_call>\n<function=get_weather>\n<parameter=city>\nOslo\n</parameter>\n"
     "<parameter=days>\n2\n</parameter>\n</function>\n</tool_call>"},
    {{DIALECT_DEEPSEEK, RENDERED_AS_DEEPSEEK, "DeepSeek V4"}, "It snows in Oslo."},
    {{DIALECT_GLM, RENDERED_AS_GLM, "GLM"}, "It snows in Oslo."},
    {{DIALECT_QWEN, RENDERED_AS_QWEN, "Qwen"}, "It snows in Oslo."},
};

static int checks;
static int disagreements;

static const char *string_in(const cJSON *object, const char *key) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(object, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static char *system_words_of(const cJSON *messages) {
    const cJSON *message;
    cJSON_ArrayForEach(message, messages) {
        const char *role = string_in(message, "role");
        if (role && strcmp(role, "system") == 0) return strdup(string_in(message, "content"));
    }
    return NULL;
}

static void hear_model(chat *c, const cJSON *message) {
    turn *spoken = chat_add_turn(c, MODEL_SAYS);
    const cJSON *call;
    if (string_in(message, "content")) turn_add_words(spoken, string_in(message, "content"), NULL);
    cJSON_ArrayForEach(call, cJSON_GetObjectItemCaseSensitive(message, "tool_calls")) {
        const cJSON *function = cJSON_GetObjectItemCaseSensitive(call, "function");
        turn_add_tendril(spoken, string_in(call, "id"), string_in(function, "name"),
                         string_in(function, "arguments"));
    }
}

static chat chat_of(const cJSON *messages) {
    chat c = {0};
    const cJSON *message;
    cJSON_ArrayForEach(message, messages) {
        const char *role = string_in(message, "role");
        if (!role || strcmp(role, "system") == 0) continue;
        if (strcmp(role, "assistant") == 0) {
            hear_model(&c, message);
        } else {
            turn *spoken = chat_add_turn(&c, strcmp(role, "tool") == 0 ? TOOL_ANSWERS : USER_SAYS);
            turn_add_words(spoken, string_in(message, "content"), NULL);
        }
    }
    return c;
}

static void report_where_they_part(const char *what, const char *ours, const char *theirs) {
    size_t at = 0;
    disagreements++;
    if (!ours || !theirs) {
        fprintf(stderr, "DISAGREE %s: %s rendered nothing\n", what, ours ? "ds4" : "the bee");
        return;
    }
    while (ours[at] && ours[at] == theirs[at]) at++;
    fprintf(stderr, "DISAGREE %s at byte %zu\n  bee: %.120s\n  ds4: %.120s\n", what, at,
            ours + (at > 40 ? at - 40 : 0), theirs + (at > 40 ? at - 40 : 0));
}

static bool ds4_orders_system_differently(const family *f, const conversation *talk, bool with_tools) {
    return f->ours == DIALECT_DEEPSEEK && talk->has_system && with_tools;
}

static void prompts_agree(const family *f, const conversation *talk, bool with_tools) {
    cJSON *messages = cJSON_Parse(talk->openai_messages);
    cJSON *tools = with_tools ? cJSON_Parse(TOOLS) : NULL;
    char *system = system_words_of(messages);
    chat c = chat_of(messages);
    char *ours = chat_as_prompt(f->ours, system, tools, &c);
    char *theirs = ds4_server_prompt(f->theirs, talk->openai_messages, with_tools ? TOOLS : NULL);
    char what[256];

    snprintf(what, sizeof what, "%s prompt for %s%s", f->name, talk->name,
             with_tools ? ", tools offered" : "");
    checks++;
    if (!ours || !theirs || strcmp(ours, theirs) != 0) report_where_they_part(what, ours, theirs);
    free(ours);
    free(theirs);
    free(system);
    chat_free(&c);
    cJSON_Delete(tools);
    cJSON_Delete(messages);
}

static cJSON *calls_the_bee_reads(dialect d, const char *grown) {
    reaching r;
    cJSON *calls;
    if (!read_tendrils(d, grown, NULL, &r)) return NULL;
    calls = cJSON_CreateArray();
    for (size_t i = 0; i < r.count; i++) {
        cJSON *call = cJSON_CreateObject();
        cJSON_AddStringToObject(call, "name", r.tendrils[i].name);
        cJSON_AddItemToObject(call, "arguments", cJSON_Parse(r.tendrils[i].args_json));
        cJSON_AddItemToArray(calls, call);
    }
    reaching_free(&r);
    return calls;
}

static void calls_agree(const sample *s) {
    cJSON *ours = calls_the_bee_reads(s->of.ours, s->grown);
    char *theirs_text = ds4_server_calls(s->of.theirs, s->grown);
    cJSON *theirs = theirs_text ? cJSON_Parse(theirs_text) : NULL;
    char what[256];

    snprintf(what, sizeof what, "%s calls read from \"%.30s...\"", s->of.name, s->grown);
    checks++;
    if ((ours || theirs) && !cJSON_Compare(ours, theirs, true)) {
        char *ours_text = ours ? cJSON_PrintUnformatted(ours) : NULL;
        disagreements++;
        fprintf(stderr, "DISAGREE %s\n  bee: %s\n  ds4: %s\n", what, ours_text ? ours_text : "none",
                theirs_text ? theirs_text : "none");
        cJSON_free(ours_text);
    }
    cJSON_Delete(ours);
    cJSON_Delete(theirs);
    free(theirs_text);
}

int main(void) {
    const family *families[] = {&DEEPSEEK, &DEEPSEEK41, &GLM, &QWEN};
    size_t f;
    size_t t;

    for (f = 0; f < sizeof families / sizeof families[0]; f++) {
        for (t = 0; t < sizeof CONVERSATIONS / sizeof CONVERSATIONS[0]; t++) {
            prompts_agree(families[f], &CONVERSATIONS[t], false);
            if (!ds4_orders_system_differently(families[f], &CONVERSATIONS[t], true)) {
                prompts_agree(families[f], &CONVERSATIONS[t], true);
            }
        }
    }
    for (t = 0; t < sizeof SAMPLES / sizeof SAMPLES[0]; t++) calls_agree(&SAMPLES[t]);
    if (disagreements) {
        fprintf(stderr, "the bee and ds4 part on %d of %d\n", disagreements, checks);
        return 1;
    }
    printf("conform: the bee renders and reads as ds4 does, %d of %d\n", checks, checks);
    return 0;
}
