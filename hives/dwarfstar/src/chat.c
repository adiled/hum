#define _POSIX_C_SOURCE 200809L

#include "chat.h"

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DEEPSEEK_BEGIN "<｜begin▁of▁sentence｜>"
#define DEEPSEEK_END_OF_TURN "<｜end▁of▁sentence｜>"
#define DEEPSEEK_SYSTEM "<｜System｜>"
#define DEEPSEEK_USER "<｜User｜>"
#define DEEPSEEK_ASSISTANT "<｜Assistant｜>"
#define THINKING_SKIPPED "</think>"

#define DSML_V4_CALLS "<｜DSML｜tool_calls>"
#define DSML_V4_INVOKE "<｜DSML｜invoke"
#define DSML_V4_PARAMETER "<｜DSML｜parameter"
#define DSML_V4_PARAMETER_END "</｜DSML｜parameter>"
#define DSML_V41_CALLS "<｜DSML｜ calls>"
#define DSML_V41_INVOKE "<｜DSML｜ invoke"
#define DSML_V41_INVOKE_END "</｜DSML｜ invoke>"
#define DSML_V41_CALLS_END "</｜DSML｜ calls>"
#define DSML_V41_PARAMETER "<｜DSML｜ parameter"
#define DSML_V41_PARAMETER_END "</｜DSML｜ parameter>"

typedef struct {
    const char *calls;
    const char *calls_end;
    const char *invoke;
    const char *invoke_end;
    const char *parameter;
    const char *parameter_end;
} dsml_spelling;

static const dsml_spelling DSML_V4 = {
    DSML_V4_CALLS, "</｜DSML｜tool_calls>", DSML_V4_INVOKE, "</｜DSML｜invoke>",
    DSML_V4_PARAMETER, DSML_V4_PARAMETER_END,
};
static const dsml_spelling DSML_V4_UNBARRED = {
    "<DSML｜tool_calls>", "</DSML｜tool_calls>", "<DSML｜invoke", "</DSML｜invoke>",
    "<DSML｜parameter", "</DSML｜parameter>",
};
static const dsml_spelling DSML_PLAIN_XML = {
    "<tool_calls>", "</tool_calls>", "<invoke", "</invoke>", "<parameter", "</parameter>",
};
static const dsml_spelling DSML_V41 = {
    DSML_V41_CALLS, DSML_V41_CALLS_END, DSML_V41_INVOKE, DSML_V41_INVOKE_END,
    DSML_V41_PARAMETER, DSML_V41_PARAMETER_END,
};

static const char *const TENDRIL_OPENINGS[] = {
    DSML_V41_CALLS, DSML_V4_CALLS, "<DSML｜tool_calls>", "<tool_calls>", "<tool_call>",
};

static const char *const ARGUMENT_WRAPPERS[][2] = {
    {DSML_V41_PARAMETER, DSML_V41_PARAMETER_END},
    {DSML_V4_PARAMETER, DSML_V4_PARAMETER_END},
    {"<DSML｜parameter", "</DSML｜parameter>"},
    {"<parameter", "</parameter>"},
    {"<arg_key>", "</arg_key>"},
    {"<arg_value>", "</arg_value>"},
};

#define COUNT_OF(array) (sizeof(array) / sizeof((array)[0]))

typedef struct {
    char *bytes;
    size_t len;
    size_t room;
    bool out_of_memory;
} page;

static void page_add_n(page *p, const char *s, size_t n) {
    if (p->out_of_memory || n == 0) return;
    if (p->len + n + 1 > p->room) {
        size_t room = p->room ? p->room : 1024;
        char *grown;
        while (room < p->len + n + 1) room *= 2;
        grown = (char *)realloc(p->bytes, room);
        if (!grown) {
            p->out_of_memory = true;
            return;
        }
        p->bytes = grown;
        p->room = room;
    }
    memcpy(p->bytes + p->len, s, n);
    p->len += n;
    p->bytes[p->len] = '\0';
}

static void page_add(page *p, const char *s) {
    if (s) page_add_n(p, s, strlen(s));
}

static void page_add_char(page *p, char c) {
    page_add_n(p, &c, 1);
}

static char *page_taken(page *p) {
    if (p->out_of_memory) {
        free(p->bytes);
        return NULL;
    }
    return p->bytes ? p->bytes : strdup("");
}

static char *copy_of(const char *s) {
    return s ? strdup(s) : NULL;
}

static char *copy_of_n(const char *s, size_t n) {
    char *copy = (char *)malloc(n + 1);
    if (!copy) return NULL;
    memcpy(copy, s, n);
    copy[n] = '\0';
    return copy;
}

static bool starts_with(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

static const char *past_space(const char *s) {
    while (*s && isspace((unsigned char)*s)) s++;
    return s;
}

static size_t without_trailing_space(const char *s, size_t n) {
    while (n > 0 && isspace((unsigned char)s[n - 1])) n--;
    return n;
}

static char *trimmed_copy(const char *from, const char *to) {
    while (from < to && isspace((unsigned char)*from)) from++;
    while (to > from && isspace((unsigned char)to[-1])) to--;
    return copy_of_n(from, (size_t)(to - from));
}

static void page_add_trimmed(page *p, const char *s) {
    const char *from = s ? s : "";
    const char *to = from + strlen(from);
    while (from < to && isspace((unsigned char)*from)) from++;
    while (to > from && isspace((unsigned char)to[-1])) to--;
    page_add_n(p, from, (size_t)(to - from));
}

static bool starts_thinking(const char *words) {
    return words && (starts_with(words, "<think>") || starts_with(words, "</think>"));
}

static bool is_escaped_closing(const char *s, const char *closing) {
    if (*s != '&') return false;
    s++;
    while (starts_with(s, "amp;")) s += 4;
    return starts_with(s, "lt;") && starts_with(s + 3, closing + 1);
}

static bool would_close_early(const char *s, const char *closing) {
    return starts_with(s, closing) || is_escaped_closing(s, closing);
}

static void page_add_guarded(page *p, const char *body, const char *closing) {
    const char *s;
    for (s = body ? body : ""; *s; s++) {
        if (would_close_early(s, closing)) {
            page_add(p, *s == '<' ? "&lt;" : "&amp;");
        } else {
            page_add_char(p, *s);
        }
    }
}

static void unguard(char *s, const char *closing) {
    char *out = s;
    while (*s) {
        if (!is_escaped_closing(s, closing)) {
            *out++ = *s++;
        } else if (starts_with(s, "&amp;")) {
            *out++ = '&';
            s += 5;
        } else {
            *out++ = '<';
            s += 4;
        }
    }
    *out = '\0';
}

static void page_add_attribute(page *p, const char *value) {
    const char *s;
    for (s = value ? value : ""; *s; s++) {
        switch (*s) {
        case '&': page_add(p, "&amp;"); break;
        case '<': page_add(p, "&lt;"); break;
        case '>': page_add(p, "&gt;"); break;
        case '"': page_add(p, "&quot;"); break;
        default: page_add_char(p, *s);
        }
    }
}

static char *attribute_unescaped(const char *from, const char *to) {
    static const char *const entities[][2] = {
        {"&amp;", "&"}, {"&lt;", "<"}, {"&gt;", ">"}, {"&quot;", "\""}, {"&apos;", "'"},
    };
    page p = {0};
    while (from < to) {
        size_t i;
        bool replaced = false;
        for (i = 0; i < COUNT_OF(entities) && *from == '&'; i++) {
            size_t n = strlen(entities[i][0]);
            if ((size_t)(to - from) >= n && strncmp(from, entities[i][0], n) == 0) {
                page_add(&p, entities[i][1]);
                from += n;
                replaced = true;
                break;
            }
        }
        if (!replaced) page_add_char(&p, *from++);
    }
    return page_taken(&p);
}

static char *attribute_in(const char *tag, const char *tag_end, const char *name) {
    char wanted[32];
    const char *at;
    const char *value;
    const char *value_end;

    snprintf(wanted, sizeof wanted, "%s=\"", name);
    for (at = tag; at < tag_end; at++) {
        if (strncmp(at, wanted, strlen(wanted)) == 0) break;
    }
    if (at >= tag_end) return NULL;
    value = at + strlen(wanted);
    value_end = memchr(value, '"', (size_t)(tag_end - value));
    return value_end ? attribute_unescaped(value, value_end) : NULL;
}

static char *json_text_of(const cJSON *item) {
    char *printed = cJSON_PrintUnformatted(item);
    char *copy = copy_of(printed);
    cJSON_free(printed);
    return copy;
}

static void page_add_json(page *p, const cJSON *item) {
    char *json = json_text_of(item);
    page_add(p, json);
    free(json);
}

static void page_add_json_string(page *p, const char *s) {
    cJSON *string = cJSON_CreateString(s ? s : "");
    page_add_json(p, string);
    cJSON_Delete(string);
}

static cJSON *whole_json_value(const char *text) {
    const char *end = NULL;
    cJSON *parsed = cJSON_ParseWithOpts(text, &end, true);
    return parsed;
}

static const cJSON *member(const cJSON *object, const char *key) {
    return cJSON_GetObjectItemCaseSensitive(object, key);
}

static const char *string_member(const cJSON *object, const char *key) {
    const cJSON *value = member(object, key);
    return cJSON_IsString(value) ? value->valuestring : NULL;
}

static const cJSON *schema_of_tool(const cJSON *tool) {
    const cJSON *function = member(tool, "function");
    if (cJSON_IsObject(function)) return member(function, "parameters");
    return cJSON_IsObject(member(tool, "inputSchema")) ? member(tool, "inputSchema")
                                                       : member(tool, "parameters");
}

static const char *name_of_tool(const cJSON *tool) {
    const cJSON *function = member(tool, "function");
    return cJSON_IsObject(function) ? string_member(function, "name") : string_member(tool, "name");
}

static cJSON *function_of_tool(const cJSON *tool) {
    const cJSON *function = member(tool, "function");
    const cJSON *schema = schema_of_tool(tool);
    const char *description = string_member(tool, "description");
    cJSON *shaped;

    if (cJSON_IsObject(function)) return cJSON_Duplicate(function, true);
    if (!name_of_tool(tool)) return NULL;
    shaped = cJSON_CreateObject();
    if (!shaped) return NULL;
    cJSON_AddStringToObject(shaped, "name", name_of_tool(tool));
    if (description) cJSON_AddStringToObject(shaped, "description", description);
    if (cJSON_IsObject(schema)) {
        cJSON_AddItemToObject(shaped, "parameters", cJSON_Duplicate(schema, true));
    } else {
        cJSON *empty = cJSON_AddObjectToObject(shaped, "parameters");
        cJSON_AddStringToObject(empty, "type", "object");
        cJSON_AddObjectToObject(empty, "properties");
    }
    return shaped;
}

static bool offers_tools(const cJSON *tools) {
    const cJSON *tool;
    cJSON_ArrayForEach(tool, tools) {
        if (name_of_tool(tool)) return true;
    }
    return false;
}

static void page_add_functions_one_per_line(page *p, const cJSON *tools) {
    const cJSON *tool;
    bool first = true;
    cJSON_ArrayForEach(tool, tools) {
        cJSON *function = function_of_tool(tool);
        if (!function) continue;
        if (!first) page_add_char(p, '\n');
        page_add_json(p, function);
        cJSON_Delete(function);
        first = false;
    }
}

static void page_add_function_spaced(page *p, const cJSON *function) {
    const cJSON *field;
    bool first = true;
    page_add_char(p, '{');
    cJSON_ArrayForEach(field, function) {
        if (!first) page_add(p, ", ");
        page_add_json_string(p, field->string);
        page_add(p, ": ");
        page_add_json(p, field);
        first = false;
    }
    page_add_char(p, '}');
}

static void page_add_deepseek_tools(page *p, const cJSON *tools, bool v41) {
    if (v41) {
        page_add(p,
            "## Tools\n\n"
            "You can invoke tools using this format:\n\n"
            DSML_V41_CALLS "\n"
            DSML_V41_INVOKE " name=\"$TOOL_NAME\">\n"
            DSML_V41_PARAMETER " name=\"$PARAMETER_NAME\" string=\"true|false\">$PARAMETER_VALUE" DSML_V41_PARAMETER_END "\n"
            DSML_V41_INVOKE_END "\n" DSML_V41_CALLS_END "\n\n"
            "String values use string=\"true\"; all other values use JSON and string=\"false\". "
            "Inside string values only, escape a literal </｜DSML｜ parameter> as &lt;/｜DSML｜ parameter>. "
            "To write that escaped spelling literally, use &amp;lt;/｜DSML｜ parameter>. Other HTML entities are unchanged.\n\n"
            "Finish reasoning with </think> before tool calls or a final response.\n\n"
            "### Available Tool Schemas\n\n");
    } else {
        page_add(p,
            "## Tools\n\n"
            "You have access to a set of tools to help answer the user question. "
            "You can invoke tools by writing a \"<｜DSML｜tool_calls>\" block like the following:\n\n"
            "<｜DSML｜tool_calls>\n"
            "<｜DSML｜invoke name=\"$TOOL_NAME\">\n"
            "<｜DSML｜parameter name=\"$PARAMETER_NAME\" string=\"true|false\">$PARAMETER_VALUE</｜DSML｜parameter>\n"
            "...\n"
            "</｜DSML｜invoke>\n"
            "<｜DSML｜invoke name=\"$TOOL_NAME2\">\n"
            "...\n"
            "</｜DSML｜invoke>\n"
            "</｜DSML｜tool_calls>\n\n"
            "String parameters should be specified as raw text and set `string=\"true\"`. "
            "Preserve characters such as `>`, `&`, and `&&` exactly; never replace normal string characters with XML or HTML entity escapes. "
            "Only if a string value itself contains the exact closing parameter tag `</｜DSML｜parameter>`, write that tag as `&lt;/｜DSML｜parameter>` inside the value. "
            "For all other types (numbers, booleans, arrays, objects), pass the value in JSON format and set `string=\"false\"`.\n\n"
            "When thinking mode is enabled, finish reasoning with </think> before any tool calls or final response.\n\n"
            "Otherwise, output directly after </think> with tool calls or final response.\n\n"
            "### Available Tool Schemas\n\n");
    }
    page_add_functions_one_per_line(p, tools);
    page_add(p, "\n\nYou MUST strictly follow the above defined tool name and parameter schemas to invoke tool calls. "
                "Use the exact parameter names from the schemas.");
}

static void page_add_glm_tools(page *p, const cJSON *tools) {
    const cJSON *tool;
    page_add(p,
        "\n# Tools\n\n"
        "You may call one or more functions to assist with the user query.\n\n"
        "You are provided with function signatures within <tools></tools> XML tags:\n"
        "<tools>\n\n");
    cJSON_ArrayForEach(tool, tools) {
        cJSON *function = function_of_tool(tool);
        if (!function) continue;
        page_add_function_spaced(p, function);
        page_add(p, "\n\n\n");
        cJSON_Delete(function);
    }
    page_add(p,
        "</tools>\n\n"
        "For each function call, output the function name and arguments within the following XML format:\n"
        "<tool_call>{function-name}"
        "<arg_key>{arg-key-1}</arg_key>"
        "<arg_value>{arg-value-1}</arg_value>"
        "<arg_key>{arg-key-2}</arg_key>"
        "<arg_value>{arg-value-2}</arg_value>"
        "...</tool_call>");
}

static void page_add_qwen_tools(page *p, const cJSON *tools) {
    const cJSON *tool;
    page_add(p, "# Tools\n\nYou have access to the following functions:\n\n<tools>");
    cJSON_ArrayForEach(tool, tools) {
        cJSON *function = function_of_tool(tool);
        if (!function) continue;
        page_add(p, "\n{\"type\": \"function\", \"function\": ");
        page_add_function_spaced(p, function);
        page_add_char(p, '}');
        cJSON_Delete(function);
    }
    page_add(p,
        "\n</tools>\n\n"
        "If you choose to call a function ONLY reply in the following format with NO suffix:\n\n"
        "<tool_call>\n<function=example_function_name>\n<parameter=example_parameter_1>\nvalue_1\n</parameter>\n"
        "<parameter=example_parameter_2>\nThis is the value for the second parameter\nthat can span\nmultiple lines\n"
        "</parameter>\n</function>\n</tool_call>\n\n<IMPORTANT>\nReminder:\n"
        "- Function calls MUST follow the specified format: an inner <function=...></function> block must be nested "
        "within <tool_call></tool_call> XML tags\n"
        "- Required parameters MUST be specified\n"
        "- You may provide optional reasoning for your function call in natural language BEFORE the function call, "
        "but NOT after\n"
        "- If there is no function call available, answer the question like normal with your current knowledge and "
        "do not tell the user about function calls\n</IMPORTANT>");
}

static void page_add_json_guarded(page *p, const cJSON *value, const char *closing) {
    char *json = json_text_of(value);
    const char *s;
    for (s = json ? json : ""; *s; s++) {
        if (starts_with(s, closing)) {
            page_add(p, "\\u003c");
        } else {
            page_add_char(p, *s);
        }
    }
    free(json);
}

static void page_add_dsml_argument(page *p, const cJSON *argument, const dsml_spelling *dsml) {
    bool is_string = cJSON_IsString(argument);
    page_add(p, dsml->parameter);
    page_add(p, " name=\"");
    page_add_attribute(p, argument->string);
    page_add(p, is_string ? "\" string=\"true\">" : "\" string=\"false\">");
    if (is_string) {
        page_add_guarded(p, argument->valuestring, dsml->parameter_end);
    } else {
        page_add_json_guarded(p, argument, dsml->parameter_end);
    }
    page_add(p, dsml->parameter_end);
    page_add_char(p, '\n');
}

static void page_add_dsml_tendrils(page *p, const turn *t, const dsml_spelling *dsml) {
    size_t i;
    if (t->tendril_count == 0) return;
    if (t->tendrils_as_sampled) {
        page_add(p, t->tendrils_as_sampled);
        return;
    }
    page_add(p, "\n\n");
    page_add(p, dsml->calls);
    page_add_char(p, '\n');
    for (i = 0; i < t->tendril_count; i++) {
        const tendril *reached = &t->tendrils[i];
        cJSON *args = cJSON_Parse(reached->args_json ? reached->args_json : "");
        page_add(p, dsml->invoke);
        page_add(p, " name=\"");
        page_add_attribute(p, reached->name);
        page_add(p, "\">\n");
        if (cJSON_IsObject(args)) {
            const cJSON *argument;
            cJSON_ArrayForEach(argument, args) page_add_dsml_argument(p, argument, dsml);
        } else {
            page_add(p, dsml->parameter);
            page_add(p, " name=\"arguments\" string=\"true\">");
            page_add_guarded(p, reached->args_json, dsml->parameter_end);
            page_add(p, dsml->parameter_end);
            page_add_char(p, '\n');
        }
        cJSON_Delete(args);
        page_add(p, dsml->invoke_end);
        page_add_char(p, '\n');
    }
    page_add(p, dsml->calls_end);
}

static char *argument_as_written(const cJSON *argument) {
    return cJSON_IsString(argument) ? copy_of(argument->valuestring) : json_text_of(argument);
}

static void page_add_glm_tendrils(page *p, const turn *t) {
    size_t i;
    if (t->tendril_count == 0) return;
    if (t->tendrils_as_sampled) {
        page_add(p, t->tendrils_as_sampled);
        return;
    }
    for (i = 0; i < t->tendril_count; i++) {
        const tendril *reached = &t->tendrils[i];
        cJSON *args = cJSON_Parse(reached->args_json ? reached->args_json : "");
        page_add(p, "<tool_call>");
        page_add(p, reached->name);
        if (cJSON_IsObject(args)) {
            const cJSON *argument;
            cJSON_ArrayForEach(argument, args) {
                char *written = argument_as_written(argument);
                page_add(p, "<arg_key>");
                page_add_guarded(p, argument->string, "</arg_key>");
                page_add(p, "</arg_key><arg_value>");
                page_add_guarded(p, written, "</arg_value>");
                page_add(p, "</arg_value>");
                free(written);
            }
        } else {
            page_add(p, "<arg_key>arguments</arg_key><arg_value>");
            page_add_guarded(p, reached->args_json, "</arg_value>");
            page_add(p, "</arg_value>");
        }
        cJSON_Delete(args);
        page_add(p, "</tool_call>");
    }
}

static void page_add_qwen_tendrils(page *p, const turn *t, bool after_words) {
    size_t i;
    if (t->tendril_count == 0) return;
    if (t->tendrils_as_sampled) {
        page_add(p, t->tendrils_as_sampled);
        return;
    }
    for (i = 0; i < t->tendril_count; i++) {
        const tendril *reached = &t->tendrils[i];
        cJSON *args = cJSON_Parse(reached->args_json ? reached->args_json : "");
        if (i > 0) {
            page_add(p, "\n<tool_call>\n<function=");
        } else {
            page_add(p, after_words ? "\n\n<tool_call>\n<function=" : "<tool_call>\n<function=");
        }
        page_add(p, reached->name);
        page_add(p, ">\n");
        if (cJSON_IsObject(args)) {
            const cJSON *argument;
            cJSON_ArrayForEach(argument, args) {
                char *written = argument_as_written(argument);
                page_add(p, "<parameter=");
                page_add_guarded(p, argument->string, ">");
                page_add(p, ">\n");
                page_add_guarded(p, written, "</parameter>");
                page_add(p, "\n</parameter>\n");
                free(written);
            }
        } else if (reached->args_json && reached->args_json[0]) {
            page_add(p, "<parameter=arguments>\n");
            page_add_guarded(p, reached->args_json, "</parameter>");
            page_add(p, "\n</parameter>\n");
        }
        cJSON_Delete(args);
        page_add(p, "</function>\n</tool_call>");
    }
}

static void page_add_tool_result(page *p, const turn *t) {
    page_add(p, "<tool_result>");
    page_add_guarded(p, t->words, "</tool_result>");
    page_add(p, "</tool_result>");
}

static char *deepseek_prompt(const char *system, const cJSON *tools, const chat *c) {
    page p = {0};
    bool model_turn_due = false;
    bool answers_open = false;
    size_t i;

    page_add(&p, DEEPSEEK_BEGIN);
    page_add(&p, system);
    if (offers_tools(tools)) {
        if (system) page_add(&p, "\n\n");
        page_add_deepseek_tools(&p, tools, false);
    }
    for (i = 0; i < c->count; i++) {
        const turn *t = &c->turns[i];
        switch (t->by) {
        case USER_SAYS:
            page_add(&p, DEEPSEEK_USER);
            page_add(&p, t->words);
            model_turn_due = true;
            answers_open = false;
            break;
        case TOOL_ANSWERS:
            if (!answers_open) page_add(&p, DEEPSEEK_USER);
            page_add_tool_result(&p, t);
            model_turn_due = true;
            answers_open = true;
            break;
        case MODEL_SAYS:
            if (model_turn_due) page_add(&p, DEEPSEEK_ASSISTANT THINKING_SKIPPED);
            page_add(&p, t->words);
            page_add_dsml_tendrils(&p, t, &DSML_V4);
            page_add(&p, DEEPSEEK_END_OF_TURN);
            model_turn_due = false;
            answers_open = false;
            break;
        }
    }
    if (model_turn_due) page_add(&p, DEEPSEEK_ASSISTANT THINKING_SKIPPED);
    return page_taken(&p);
}

static char *deepseek41_prompt(const char *system, const cJSON *tools, const chat *c) {
    page p = {0};
    bool model_turn_due = false;
    bool user_open = false;
    size_t i;

    page_add(&p, DEEPSEEK_BEGIN);
    if (offers_tools(tools)) {
        page_add(&p, DEEPSEEK_SYSTEM);
        page_add_deepseek_tools(&p, tools, true);
        if (system) page_add(&p, "\n\n");
    } else if (system) {
        page_add(&p, DEEPSEEK_SYSTEM);
    }
    page_add(&p, system);
    for (i = 0; i < c->count; i++) {
        const turn *t = &c->turns[i];
        if (t->by == MODEL_SAYS) {
            if (model_turn_due) page_add(&p, DEEPSEEK_ASSISTANT THINKING_SKIPPED);
            page_add(&p, t->words);
            page_add_dsml_tendrils(&p, t, &DSML_V41);
            page_add(&p, DEEPSEEK_END_OF_TURN);
            model_turn_due = false;
            user_open = false;
            continue;
        }
        page_add(&p, user_open ? "\n\n" : DEEPSEEK_USER);
        if (t->by == TOOL_ANSWERS) {
            page_add_tool_result(&p, t);
        } else {
            page_add(&p, t->words);
        }
        model_turn_due = true;
        user_open = true;
    }
    if (model_turn_due) page_add(&p, DEEPSEEK_ASSISTANT THINKING_SKIPPED);
    return page_taken(&p);
}

static char *glm_prompt(const char *system, const cJSON *tools, const chat *c) {
    page p = {0};
    bool model_turn_due = false;
    bool observing = false;
    size_t i;

    page_add(&p, "[gMASK]<sop>");
    if (offers_tools(tools)) {
        page_add(&p, "<|system|>");
        page_add_glm_tools(&p, tools);
    }
    if (system) {
        page_add(&p, "<|system|>");
        page_add(&p, system);
    }
    for (i = 0; i < c->count; i++) {
        const turn *t = &c->turns[i];
        switch (t->by) {
        case TOOL_ANSWERS:
            if (!observing) page_add(&p, "<|observation|>");
            page_add(&p, "<tool_response>");
            page_add_guarded(&p, t->words, "</tool_response>");
            page_add(&p, "</tool_response>");
            observing = true;
            model_turn_due = true;
            break;
        case USER_SAYS:
            page_add(&p, "<|user|>");
            page_add(&p, t->words);
            observing = false;
            model_turn_due = true;
            break;
        case MODEL_SAYS:
            page_add(&p, "<|assistant|>");
            if (!starts_thinking(t->words)) page_add(&p, "<think></think>");
            page_add_trimmed(&p, t->words);
            page_add_glm_tendrils(&p, t);
            observing = false;
            model_turn_due = false;
            break;
        }
    }
    if (model_turn_due) page_add(&p, "<|assistant|><think></think>");
    return page_taken(&p);
}

static void page_add_qwen_model_turn(page *p, const turn *t) {
    const char *words = t->words ? t->words : "";
    page body = {0};
    bool has_words;

    page_add_trimmed(&body, words);
    has_words = body.len > 0;
    if (has_words && t->tendril_count == 0) {
        const char *end = words + strlen(words);
        while (end > words && isspace((unsigned char)end[-1])) end--;
        page_add(&body, end);
    }
    page_add(p, "<|im_start|>assistant\n");
    if (!starts_thinking(words)) page_add(p, "<think>\n\n</think>\n\n");
    page_add(p, body.bytes);
    free(body.bytes);
    page_add_qwen_tendrils(p, t, has_words);
    page_add(p, "<|im_end|>\n");
}

static char *qwen_prompt(const char *system, const cJSON *tools, const chat *c) {
    page p = {0};
    page trimmed_system = {0};
    bool model_turn_due = false;
    bool answers_open = false;
    size_t i;

    page_add_trimmed(&trimmed_system, system);
    if (offers_tools(tools) || trimmed_system.len > 0) {
        page_add(&p, "<|im_start|>system\n");
        if (offers_tools(tools)) {
            page_add_qwen_tools(&p, tools);
            if (trimmed_system.len > 0) page_add(&p, "\n\n");
        }
        page_add(&p, trimmed_system.bytes);
        page_add(&p, "<|im_end|>\n");
    }
    free(trimmed_system.bytes);
    for (i = 0; i < c->count; i++) {
        const turn *t = &c->turns[i];
        if (t->by == TOOL_ANSWERS) {
            if (!answers_open) page_add(&p, "<|im_start|>user");
            page_add(&p, "\n<tool_response>\n");
            page_add_guarded(&p, t->words, "</tool_response>");
            page_add(&p, "\n</tool_response>");
            answers_open = true;
            model_turn_due = true;
            continue;
        }
        if (answers_open) page_add(&p, "<|im_end|>\n");
        answers_open = false;
        if (t->by == USER_SAYS) {
            page_add(&p, "<|im_start|>user\n");
            page_add_trimmed(&p, t->words);
            page_add(&p, "<|im_end|>\n");
            model_turn_due = true;
        } else {
            page_add_qwen_model_turn(&p, t);
            model_turn_due = false;
        }
    }
    if (answers_open) page_add(&p, "<|im_end|>\n");
    if (model_turn_due) page_add(&p, "<|im_start|>assistant\n<think>\n\n</think>\n\n");
    return page_taken(&p);
}

char *chat_as_prompt(dialect d, const char *system, const cJSON *tools, const chat *c) {
    if (system && !system[0]) system = NULL;
    switch (d) {
    case DIALECT_DEEPSEEK41: return deepseek41_prompt(system, tools, c);
    case DIALECT_GLM: return glm_prompt(system, tools, c);
    case DIALECT_QWEN: return qwen_prompt(system, tools, c);
    case DIALECT_DEEPSEEK: break;
    }
    return deepseek_prompt(system, tools, c);
}

static size_t longest_opening_begun_at_end(const char *grown, size_t len) {
    size_t longest = 0;
    size_t i;
    for (i = 0; i < COUNT_OF(TENDRIL_OPENINGS); i++) {
        size_t k = strlen(TENDRIL_OPENINGS[i]) - 1;
        if (k > len) k = len;
        for (; k > longest; k--) {
            if (memcmp(grown + len - k, TENDRIL_OPENINGS[i], k) == 0) {
                longest = k;
                break;
            }
        }
    }
    return longest;
}

size_t words_safe_to_show(const char *grown, bool *tendrils_began) {
    size_t len = strlen(grown);
    const char *earliest = NULL;
    size_t i;
    for (i = 0; i < COUNT_OF(TENDRIL_OPENINGS); i++) {
        const char *at = strstr(grown, TENDRIL_OPENINGS[i]);
        if (at && (!earliest || at < earliest)) earliest = at;
    }
    if (earliest) {
        *tendrils_began = true;
        return without_trailing_space(grown, (size_t)(earliest - grown));
    }
    return without_trailing_space(grown, len - longest_opening_begun_at_end(grown, len));
}

static bool reaching_add(reaching *r, char *name, cJSON *args) {
    tendril *grown = (tendril *)realloc(r->tendrils, (r->count + 1) * sizeof *grown);
    char *args_json = json_text_of(args);
    if (!grown || !args_json) {
        if (grown) r->tendrils = grown;
        free(args_json);
        return false;
    }
    r->tendrils = grown;
    r->tendrils[r->count].call_id = NULL;
    r->tendrils[r->count].name = name;
    r->tendrils[r->count].args_json = args_json;
    r->count++;
    return true;
}

static void add_argument_as_json_or_words(cJSON *args, const char *key, const char *value) {
    cJSON *parsed = whole_json_value(value);
    if (parsed) {
        cJSON_AddItemToObject(args, key, parsed);
    } else {
        cJSON_AddStringToObject(args, key, value);
    }
}

static bool schema_says_string(const cJSON *tools, const char *tool_name, const char *key) {
    const cJSON *tool;
    cJSON_ArrayForEach(tool, tools) {
        const char *name = name_of_tool(tool);
        const cJSON *properties;
        if (!name || strcmp(name, tool_name) != 0) continue;
        properties = member(schema_of_tool(tool), "properties");
        return cJSON_IsObject(properties) && string_member(member(properties, key), "type") &&
               strcmp(string_member(member(properties, key), "type"), "string") == 0;
    }
    return false;
}

static void add_argument_typed(cJSON *args, const cJSON *tools, const char *tool_name,
                               const char *key, char *value, const char *closing_if_words) {
    cJSON *parsed = schema_says_string(tools, tool_name, key) ? NULL : whole_json_value(value);
    if (parsed && !cJSON_IsString(parsed)) {
        cJSON_AddItemToObject(args, key, parsed);
        return;
    }
    cJSON_Delete(parsed);
    if (closing_if_words) unguard(value, closing_if_words);
    cJSON_AddStringToObject(args, key, value);
}

static const char *opening_found(const char *grown, const char *opening) {
    char after_gap[64];
    const char *at;
    snprintf(after_gap, sizeof after_gap, "\n\n%s", opening);
    at = strstr(grown, after_gap);
    return at ? at : strstr(grown, opening);
}

static bool read_dsml_leaf(const char **cursor, const dsml_spelling *dsml, cJSON *into) {
    const char *p = *cursor;
    const char *tag_end = strchr(p, '>');
    const char *value_end;
    char *name;
    char *is_string;
    char *value;

    if (!starts_with(p, dsml->parameter) || !tag_end) return false;
    name = attribute_in(p, tag_end, "name");
    is_string = attribute_in(p, tag_end, "string");
    value_end = strstr(tag_end + 1, dsml->parameter_end);
    if (!name || !value_end) {
        free(name);
        free(is_string);
        return false;
    }
    value = copy_of_n(tag_end + 1, (size_t)(value_end - tag_end - 1));
    if (!is_string || strcmp(is_string, "true") == 0) {
        unguard(value, dsml->parameter_end);
        cJSON_AddStringToObject(into, name, value);
    } else {
        add_argument_as_json_or_words(into, name, value);
    }
    free(name);
    free(is_string);
    free(value);
    *cursor = value_end + strlen(dsml->parameter_end);
    return true;
}

static bool read_dsml_nested(const char **cursor, const dsml_spelling *dsml, cJSON *into) {
    const char *p = *cursor;
    bool any = false;
    for (;;) {
        p = past_space(p);
        if (!starts_with(p, dsml->parameter)) break;
        if (!read_dsml_leaf(&p, dsml, into)) return false;
        any = true;
    }
    *cursor = p;
    return any;
}

static bool read_dsml_arguments(const char **cursor, const dsml_spelling *dsml, cJSON *args) {
    const char *p = *cursor;
    for (;;) {
        const char *tag_end;
        const char *value_start;
        char *name;
        char *is_string;

        p = past_space(p);
        if (starts_with(p, dsml->invoke_end)) {
            *cursor = p + strlen(dsml->invoke_end);
            return true;
        }
        tag_end = strchr(p, '>');
        if (!starts_with(p, dsml->parameter) || !tag_end) return false;
        name = attribute_in(p, tag_end, "name");
        is_string = attribute_in(p, tag_end, "string");
        value_start = tag_end + 1;
        if (name && !is_string && starts_with(past_space(value_start), dsml->parameter)) {
            cJSON *nested = cJSON_CreateObject();
            if (!read_dsml_nested(&value_start, dsml, nested)) {
                cJSON_Delete(nested);
                free(name);
                return false;
            }
            cJSON_AddItemToObject(args, name, nested);
            free(name);
            p = past_space(value_start);
            if (starts_with(p, dsml->parameter_end)) p += strlen(dsml->parameter_end);
            continue;
        }
        free(name);
        free(is_string);
        if (!read_dsml_leaf(&p, dsml, args)) return false;
    }
}

static bool read_dsml(const char *grown, reaching *out) {
    static const dsml_spelling *const v4_spellings[] = {&DSML_V4, &DSML_V4_UNBARRED, &DSML_PLAIN_XML};
    const dsml_spelling *dsml = NULL;
    const char *start = NULL;
    const char *v41_start = opening_found(grown, DSML_V41.calls);
    const char *p;
    size_t i;

    for (i = 0; i < COUNT_OF(v4_spellings) && !start; i++) {
        start = opening_found(grown, v4_spellings[i]->calls);
        dsml = v4_spellings[i];
    }
    if (v41_start && (!start || v41_start < start)) {
        start = v41_start;
        dsml = &DSML_V41;
    }
    if (!start) return false;
    out->words_end = without_trailing_space(grown, (size_t)(start - grown));
    out->tendrils_start = (size_t)(start - grown);
    p = strstr(start, dsml->calls) + strlen(dsml->calls);
    for (;;) {
        const char *tag_end;
        char *name;
        cJSON *args;

        p = past_space(p);
        if (starts_with(p, dsml->calls_end)) {
            out->tendrils_end = (size_t)(p + strlen(dsml->calls_end) - grown);
            return out->count > 0;
        }
        tag_end = strchr(p, '>');
        if (!starts_with(p, dsml->invoke) || !tag_end) return false;
        name = attribute_in(p, tag_end, "name");
        if (!name) return false;
        p = tag_end + 1;
        args = cJSON_CreateObject();
        if (!read_dsml_arguments(&p, dsml, args) || !reaching_add(out, name, args)) {
            cJSON_Delete(args);
            free(name);
            return false;
        }
        cJSON_Delete(args);
    }
}

static const char *found_outside_arguments(const char *s, const char *needle) {
    while (*s) {
        size_t i;
        bool skipped = false;
        if (starts_with(s, needle)) return s;
        for (i = 0; i < COUNT_OF(ARGUMENT_WRAPPERS); i++) {
            const char *tag_end;
            const char *closing;
            if (!starts_with(s, ARGUMENT_WRAPPERS[i][0])) continue;
            tag_end = strchr(s, '>');
            closing = tag_end ? strstr(tag_end + 1, ARGUMENT_WRAPPERS[i][1]) : NULL;
            if (!closing) return NULL;
            s = closing + strlen(ARGUMENT_WRAPPERS[i][1]);
            skipped = true;
            break;
        }
        if (!skipped) s++;
    }
    return NULL;
}

static bool read_glm_arguments(const char **cursor, const char *close, const cJSON *tools,
                               const char *tool_name, cJSON *args) {
    const char *p = *cursor;
    for (;;) {
        const char *key_end;
        const char *value_end;
        char *key;
        char *value;

        p = past_space(p);
        if (starts_with(p, "</tool_call>")) {
            *cursor = p + strlen("</tool_call>");
            return true;
        }
        if (!starts_with(p, "<arg_key>")) return false;
        p += strlen("<arg_key>");
        key_end = strstr(p, "</arg_key>");
        if (!key_end || key_end > close) return false;
        key = trimmed_copy(p, key_end);
        unguard(key, "</arg_key>");
        p = past_space(key_end + strlen("</arg_key>"));
        if (!starts_with(p, "<arg_value>")) {
            free(key);
            return false;
        }
        p += strlen("<arg_value>");
        value_end = strstr(p, "</arg_value>");
        if (!value_end || value_end > close) {
            free(key);
            return false;
        }
        value = copy_of_n(p, (size_t)(value_end - p));
        unguard(value, "</arg_value>");
        add_argument_typed(args, tools, tool_name, key, value, NULL);
        free(key);
        free(value);
        p = value_end + strlen("</arg_value>");
    }
}

static bool read_glm(const char *grown, const cJSON *tools, reaching *out) {
    const char *start = strstr(grown, "<tool_call>");
    const char *block = start;
    const char *p = start;

    if (!start) return false;
    if (block >= grown + 2 && block[-2] == '\n' && block[-1] == '\n') block -= 2;
    out->words_end = without_trailing_space(grown, (size_t)(block - grown));
    out->tendrils_start = (size_t)(block - grown);
    for (;;) {
        const char *close;
        const char *first_key;
        const char *name_end;
        char *name;
        cJSON *args;

        p = past_space(p);
        if (!starts_with(p, "<tool_call>")) break;
        p += strlen("<tool_call>");
        close = found_outside_arguments(p, "</tool_call>");
        if (!close) return false;
        first_key = strstr(p, "<arg_key>");
        name_end = first_key && first_key < close ? first_key : close;
        name = trimmed_copy(p, name_end);
        if (!name || !name[0]) {
            free(name);
            return false;
        }
        p = name_end;
        args = cJSON_CreateObject();
        if (!read_glm_arguments(&p, close, tools, name, args) || !reaching_add(out, name, args)) {
            cJSON_Delete(args);
            free(name);
            return false;
        }
        cJSON_Delete(args);
        if (!starts_with(past_space(p), "<tool_call>")) {
            p = past_space(p);
            break;
        }
    }
    out->tendrils_end = (size_t)(p - grown);
    return out->count > 0;
}

static char *qwen_value(const char *from, const char *to) {
    if (from < to && *from == '\n') from++;
    if (to > from && to[-1] == '\n') to--;
    return copy_of_n(from, (size_t)(to - from));
}

static bool read_qwen_arguments(const char **cursor, const cJSON *tools, const char *tool_name,
                                cJSON *args) {
    const char *p = *cursor;
    for (;;) {
        const char *q = past_space(p);
        const char *key_end;
        const char *value_end;
        char *key;
        char *value;

        if (starts_with(q, "</function>")) {
            *cursor = q + strlen("</function>");
            return true;
        }
        if (!starts_with(q, "<parameter=")) return false;
        q += strlen("<parameter=");
        key_end = q + strcspn(q, "<>");
        if (*key_end != '>') return false;
        value_end = strstr(key_end + 1, "</parameter>");
        if (!value_end) return false;
        key = trimmed_copy(q, key_end);
        value = qwen_value(key_end + 1, value_end);
        add_argument_typed(args, tools, tool_name, key, value, "</parameter>");
        free(key);
        free(value);
        p = value_end + strlen("</parameter>");
    }
}

static bool read_qwen(const char *grown, const cJSON *tools, reaching *out) {
    const char *start = strstr(grown, "<tool_call>");
    const char *block = start;
    const char *p = start;

    if (!start) return false;
    while (block > grown && (block[-1] == '\n' || block[-1] == ' ')) block--;
    out->words_end = without_trailing_space(grown, (size_t)(block - grown));
    out->tendrils_start = (size_t)(block - grown);
    for (;;) {
        const char *name_end;
        char *name;
        cJSON *args;

        p = past_space(p);
        if (!starts_with(p, "<tool_call>")) break;
        p = past_space(p + strlen("<tool_call>"));
        if (!starts_with(p, "<function=")) return false;
        p += strlen("<function=");
        name_end = p + strcspn(p, "<>");
        if (*name_end != '>') return false;
        name = trimmed_copy(p, name_end);
        if (!name || !name[0]) {
            free(name);
            return false;
        }
        p = name_end + 1;
        args = cJSON_CreateObject();
        if (!read_qwen_arguments(&p, tools, name, args)) {
            cJSON_Delete(args);
            free(name);
            return false;
        }
        p = past_space(p);
        if (!starts_with(p, "</tool_call>") || !reaching_add(out, name, args)) {
            cJSON_Delete(args);
            free(name);
            return false;
        }
        cJSON_Delete(args);
        p += strlen("</tool_call>");
        if (!starts_with(past_space(p), "<tool_call>")) {
            p = past_space(p);
            break;
        }
    }
    out->tendrils_end = (size_t)(p - grown);
    return out->count > 0;
}

bool read_tendrils(dialect d, const char *grown, const cJSON *tools, reaching *out) {
    bool read;
    memset(out, 0, sizeof *out);
    switch (d) {
    case DIALECT_GLM: read = read_glm(grown, tools, out); break;
    case DIALECT_QWEN: read = read_qwen(grown, tools, out); break;
    case DIALECT_DEEPSEEK:
    case DIALECT_DEEPSEEK41:
    default: read = read_dsml(grown, out); break;
    }
    if (!read) reaching_free(out);
    return read;
}

void reaching_free(reaching *r) {
    size_t i;
    for (i = 0; i < r->count; i++) {
        free(r->tendrils[i].call_id);
        free(r->tendrils[i].name);
        free(r->tendrils[i].args_json);
    }
    free(r->tendrils);
    memset(r, 0, sizeof *r);
}

turn *chat_add_turn(chat *c, speaker by) {
    turn *t;
    if (c->count == c->room) {
        size_t room = c->room ? c->room * 2 : 8;
        turn *grown = (turn *)realloc(c->turns, room * sizeof *grown);
        if (!grown) return NULL;
        c->turns = grown;
        c->room = room;
    }
    t = &c->turns[c->count++];
    memset(t, 0, sizeof *t);
    t->by = by;
    return t;
}

turn *chat_last_turn(const chat *c) {
    return c->count > 0 ? &c->turns[c->count - 1] : NULL;
}

int turn_add_words(turn *t, const char *words, const char *joiner) {
    size_t had;
    size_t joining;
    char *grown;

    if (!words) return 0;
    if (!t->words) {
        t->words = copy_of(words);
        return t->words ? 0 : -1;
    }
    had = strlen(t->words);
    joining = joiner ? strlen(joiner) : 0;
    grown = (char *)realloc(t->words, had + joining + strlen(words) + 1);
    if (!grown) return -1;
    memcpy(grown + had, joiner ? joiner : "", joining);
    strcpy(grown + had + joining, words);
    t->words = grown;
    return 0;
}

int turn_add_tendril(turn *t, const char *call_id, const char *name, const char *args_json) {
    tendril *grown = (tendril *)realloc(t->tendrils, (t->tendril_count + 1) * sizeof *grown);
    if (!grown) return -1;
    t->tendrils = grown;
    grown[t->tendril_count].call_id = copy_of(call_id);
    grown[t->tendril_count].name = copy_of(name);
    grown[t->tendril_count].args_json = copy_of(args_json ? args_json : "{}");
    t->tendril_count++;
    return 0;
}

void chat_free(chat *c) {
    size_t i;
    size_t k;
    for (i = 0; i < c->count; i++) {
        turn *t = &c->turns[i];
        for (k = 0; k < t->tendril_count; k++) {
            free(t->tendrils[k].call_id);
            free(t->tendrils[k].name);
            free(t->tendrils[k].args_json);
        }
        free(t->tendrils);
        free(t->words);
        free(t->answering_call_id);
        free(t->tendrils_as_sampled);
    }
    free(c->turns);
    memset(c, 0, sizeof *c);
}
