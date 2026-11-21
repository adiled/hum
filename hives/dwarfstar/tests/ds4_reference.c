#define DS4_SERVER_TEST
#define DS4_SERVER_TEST_NO_MAIN
#include "ds4_server.c"

#include "ds4_reference.h"

static server_model_syntax syntax_of(reference_family family) {
    switch (family) {
    case RENDERED_AS_DEEPSEEK41: return SERVER_MODEL_SYNTAX_DEEPSEEK41;
    case RENDERED_AS_GLM: return SERVER_MODEL_SYNTAX_GLM;
    case RENDERED_AS_QWEN: return SERVER_MODEL_SYNTAX_QWEN;
    case RENDERED_AS_DEEPSEEK: break;
    }
    return SERVER_MODEL_SYNTAX_DEEPSEEK;
}

char *ds4_server_prompt(reference_family family, const char *openai_messages,
                        const char *openai_tools) {
    const char *messages_at = openai_messages;
    const char *tools_at = openai_tools;
    chat_msgs msgs = {0};
    tool_schema_orders orders = {0};
    char *schemas = NULL;
    char *prompt = NULL;

    if (parse_messages(&messages_at, &msgs) &&
        (!openai_tools || parse_tools_value(&tools_at, &schemas, &orders))) {
        prompt = render_chat_prompt_text_for_syntax(syntax_of(family), &msgs, schemas, &orders,
                                                    DS4_THINK_NONE);
    }
    chat_msgs_free(&msgs);
    tool_schema_orders_free(&orders);
    free(schemas);
    return prompt;
}

char *ds4_server_calls(reference_family family, const char *grown) {
    tool_calls calls = {0};
    char *words = NULL;
    char *reasoning = NULL;
    buf listed = {0};
    bool read = parse_generated_message_ex_for_syntax(syntax_of(family), grown, false, &words,
                                                      &reasoning, &calls);

    free(words);
    free(reasoning);
    if (!read || calls.len == 0) {
        tool_calls_free(&calls);
        return NULL;
    }
    buf_putc(&listed, '[');
    for (int i = 0; i < calls.len; i++) {
        if (i > 0) buf_putc(&listed, ',');
        buf_puts(&listed, "{\"name\":");
        json_escape(&listed, calls.v[i].name);
        buf_puts(&listed, ",\"arguments\":");
        buf_puts(&listed, calls.v[i].arguments ? calls.v[i].arguments : "null");
        buf_putc(&listed, '}');
    }
    buf_putc(&listed, ']');
    tool_calls_free(&calls);
    return buf_take(&listed);
}
