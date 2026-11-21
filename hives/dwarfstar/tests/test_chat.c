#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "chat.h"

static int failures;

static void expect(bool ok, const char *what, int line) {
    if (ok) return;
    failures++;
    fprintf(stderr, "test_chat.c:%d: expected %s\n", line, what);
}

static void expect_same(const char *got, const char *want, int line) {
    if (got && strcmp(got, want) == 0) return;
    failures++;
    fprintf(stderr, "test_chat.c:%d: got\n---\n%s\n---\nwanted\n---\n%s\n---\n", line,
            got ? got : "(null)", want);
}

static void expect_ends_with(const char *got, const char *tail, int line) {
    size_t n = got ? strlen(got) : 0;
    size_t k = strlen(tail);
    if (got && n >= k && strcmp(got + n - k, tail) == 0) return;
    failures++;
    fprintf(stderr, "test_chat.c:%d: got\n---\n%s\n---\nwhich does not end with\n---\n%s\n---\n",
            line, got ? got : "(null)", tail);
}

static bool starts_with(const char *text, const char *prefix) {
    return text && strncmp(text, prefix, strlen(prefix)) == 0;
}

#define EXPECT(condition) expect((condition), #condition, __LINE__)
#define EXPECT_SAME(got, want) expect_same((got), (want), __LINE__)
#define EXPECT_ENDS_WITH(got, tail) expect_ends_with((got), (tail), __LINE__)

static const char *const WEATHER_TOOL =
    "[{\"name\":\"weather\",\"description\":\"what the sky does\","
    "\"inputSchema\":{\"type\":\"object\",\"properties\":{\"city\":{\"type\":\"string\"},"
    "\"days\":{\"type\":\"integer\"}}}}]";

static void say(chat *c, speaker by, const char *words) {
    turn_add_words(chat_add_turn(c, by), words, NULL);
}

static void ask_weather_and_hear_snow(chat *c) {
    turn *checking;
    say(c, USER_SAYS, "weather in oslo?");
    checking = chat_add_turn(c, MODEL_SAYS);
    turn_add_words(checking, "checking", NULL);
    turn_add_tendril(checking, "call_1", "weather", "{\"city\":\"Oslo\",\"days\":2}");
    say(c, TOOL_ANSWERS, "snow");
}

static void deepseek_offers_tools_after_the_system_words(void) {
    cJSON *tools = cJSON_Parse(WEATHER_TOOL);
    chat c = {0};
    char *prompt;

    say(&c, USER_SAYS, "hi");
    prompt = chat_as_prompt(DIALECT_DEEPSEEK, "be brief", tools, &c);
    EXPECT(starts_with(prompt, "<｜begin▁of▁sentence｜>be brief\n\n## Tools\n\nYou have access"));
    EXPECT(strstr(prompt, "### Available Tool Schemas\n\n"
                          "{\"name\":\"weather\",\"description\":\"what the sky does\","
                          "\"parameters\":{\"type\":\"object\",\"properties\":{\"city\":{\"type\":\"string\"},"
                          "\"days\":{\"type\":\"integer\"}}}}\n\nYou MUST strictly follow") != NULL);
    EXPECT_ENDS_WITH(prompt, "Use the exact parameter names from the schemas."
                             "<｜User｜>hi<｜Assistant｜></think>");
    free(prompt);
    chat_free(&c);
    cJSON_Delete(tools);
}

static void deepseek_renders_a_graft_tendril_and_grows_on_from_its_answer(void) {
    chat c = {0};
    char *prompt;

    ask_weather_and_hear_snow(&c);
    prompt = chat_as_prompt(DIALECT_DEEPSEEK, NULL, NULL, &c);
    EXPECT_SAME(prompt,
                "<｜begin▁of▁sentence｜><｜User｜>weather in oslo?<｜Assistant｜></think>checking\n\n"
                "<｜DSML｜tool_calls>\n"
                "<｜DSML｜invoke name=\"weather\">\n"
                "<｜DSML｜parameter name=\"city\" string=\"true\">Oslo</｜DSML｜parameter>\n"
                "<｜DSML｜parameter name=\"days\" string=\"false\">2</｜DSML｜parameter>\n"
                "</｜DSML｜invoke>\n"
                "</｜DSML｜tool_calls><｜end▁of▁sentence｜>"
                "<｜User｜><tool_result>snow</tool_result><｜Assistant｜></think>");
    free(prompt);
    chat_free(&c);
}

static void deepseek_renders_sampled_tendrils_as_they_were_grown(void) {
    chat c = {0};
    turn *checking;
    char *prompt;

    say(&c, USER_SAYS, "q");
    checking = chat_add_turn(&c, MODEL_SAYS);
    turn_add_tendril(checking, "call_1", "weather", "{\"city\":\"Oslo\"}");
    checking->tendrils_as_sampled = strdup("\n\n<｜DSML｜tool_calls>AS GROWN</｜DSML｜tool_calls>");
    say(&c, TOOL_ANSWERS, "a < b && c");
    prompt = chat_as_prompt(DIALECT_DEEPSEEK, NULL, NULL, &c);
    EXPECT_SAME(prompt, "<｜begin▁of▁sentence｜><｜User｜>q<｜Assistant｜></think>\n\n"
                        "<｜DSML｜tool_calls>AS GROWN</｜DSML｜tool_calls><｜end▁of▁sentence｜>"
                        "<｜User｜><tool_result>a < b && c</tool_result><｜Assistant｜></think>");
    free(prompt);
    chat_free(&c);
}

static void deepseek41_keeps_answers_in_one_user_turn(void) {
    chat c = {0};
    turn *checking;
    char *prompt;

    say(&c, USER_SAYS, "q");
    checking = chat_add_turn(&c, MODEL_SAYS);
    turn_add_tendril(checking, "call_1", "a", "{}");
    turn_add_tendril(checking, "call_2", "b", "{}");
    say(&c, TOOL_ANSWERS, "one");
    say(&c, TOOL_ANSWERS, "two");
    prompt = chat_as_prompt(DIALECT_DEEPSEEK41, "sys", NULL, &c);
    EXPECT_SAME(prompt, "<｜begin▁of▁sentence｜><｜System｜>sys<｜User｜>q<｜Assistant｜></think>\n\n"
                        "<｜DSML｜ calls>\n"
                        "<｜DSML｜ invoke name=\"a\">\n</｜DSML｜ invoke>\n"
                        "<｜DSML｜ invoke name=\"b\">\n</｜DSML｜ invoke>\n"
                        "</｜DSML｜ calls><｜end▁of▁sentence｜>"
                        "<｜User｜><tool_result>one</tool_result>\n\n<tool_result>two</tool_result>"
                        "<｜Assistant｜></think>");
    free(prompt);
    chat_free(&c);
}

static void deepseek41_puts_tools_and_system_in_one_system_turn(void) {
    cJSON *tools = cJSON_Parse(WEATHER_TOOL);
    chat c = {0};
    char *prompt;

    say(&c, USER_SAYS, "hi");
    prompt = chat_as_prompt(DIALECT_DEEPSEEK41, "sys", tools, &c);
    EXPECT(starts_with(prompt, "<｜begin▁of▁sentence｜><｜System｜>## Tools\n\nYou can invoke tools"));
    EXPECT_ENDS_WITH(prompt, "from the schemas.\n\nsys<｜User｜>hi<｜Assistant｜></think>");
    free(prompt);
    chat_free(&c);
    cJSON_Delete(tools);
}

static void glm_renders_tendrils_inline_and_answers_as_observations(void) {
    chat c = {0};
    char *prompt;

    ask_weather_and_hear_snow(&c);
    prompt = chat_as_prompt(DIALECT_GLM, "sys", NULL, &c);
    EXPECT_SAME(prompt, "[gMASK]<sop><|system|>sys<|user|>weather in oslo?"
                        "<|assistant|><think></think>checking"
                        "<tool_call>weather<arg_key>city</arg_key><arg_value>Oslo</arg_value>"
                        "<arg_key>days</arg_key><arg_value>2</arg_value></tool_call>"
                        "<|observation|><tool_response>snow</tool_response>"
                        "<|assistant|><think></think>");
    free(prompt);
    chat_free(&c);
}

static void glm_lists_tools_spaced_in_a_system_turn(void) {
    cJSON *tools = cJSON_Parse(WEATHER_TOOL);
    chat c = {0};
    char *prompt;

    say(&c, USER_SAYS, "hi");
    prompt = chat_as_prompt(DIALECT_GLM, NULL, tools, &c);
    EXPECT(starts_with(prompt, "[gMASK]<sop><|system|>\n# Tools\n\n"));
    EXPECT(strstr(prompt, "<tools>\n\n{\"name\": \"weather\", \"description\": \"what the sky does\", "
                          "\"parameters\": {\"type\":\"object\",") != NULL);
    EXPECT_ENDS_WITH(prompt, "...</tool_call><|user|>hi<|assistant|><think></think>");
    free(prompt);
    chat_free(&c);
}

static void qwen_renders_its_chatml_with_tool_responses(void) {
    chat c = {0};
    char *prompt;

    ask_weather_and_hear_snow(&c);
    prompt = chat_as_prompt(DIALECT_QWEN, "sys", NULL, &c);
    EXPECT_SAME(prompt, "<|im_start|>system\nsys<|im_end|>\n"
                        "<|im_start|>user\nweather in oslo?<|im_end|>\n"
                        "<|im_start|>assistant\n<think>\n\n</think>\n\nchecking\n\n"
                        "<tool_call>\n<function=weather>\n"
                        "<parameter=city>\nOslo\n</parameter>\n"
                        "<parameter=days>\n2\n</parameter>\n"
                        "</function>\n</tool_call><|im_end|>\n"
                        "<|im_start|>user\n<tool_response>\nsnow\n</tool_response><|im_end|>\n"
                        "<|im_start|>assistant\n<think>\n\n</think>\n\n");
    free(prompt);
    chat_free(&c);
}

static void a_dsml_block_is_read_into_tendrils(void) {
    const char *grown =
        "let me look\n\n<｜DSML｜tool_calls>\n"
        "<｜DSML｜invoke name=\"weather\">\n"
        "<｜DSML｜parameter name=\"city\" string=\"true\">Oslo &amp; Bergen</｜DSML｜parameter>\n"
        "<｜DSML｜parameter name=\"days\" string=\"false\">2</｜DSML｜parameter>\n"
        "</｜DSML｜invoke>\n"
        "<｜DSML｜invoke name=\"clock\">\n"
        "</｜DSML｜invoke>\n"
        "</｜DSML｜tool_calls>";
    reaching r;

    EXPECT(read_tendrils(DIALECT_DEEPSEEK, grown, NULL, &r));
    EXPECT(r.count == 2);
    EXPECT(r.words_end == strlen("let me look"));
    EXPECT(r.tendrils_start == strlen("let me look"));
    EXPECT(r.tendrils_end == strlen(grown));
    EXPECT_SAME(r.tendrils[0].name, "weather");
    EXPECT_SAME(r.tendrils[0].args_json, "{\"city\":\"Oslo &amp; Bergen\",\"days\":2}");
    EXPECT_SAME(r.tendrils[1].name, "clock");
    EXPECT_SAME(r.tendrils[1].args_json, "{}");
    reaching_free(&r);
}

static void a_dsml_closing_tag_inside_a_string_survives_the_round_trip(void) {
    chat c = {0};
    turn *writing = NULL;
    char *prompt;
    const char *block;
    reaching r;

    say(&c, USER_SAYS, "q");
    writing = chat_add_turn(&c, MODEL_SAYS);
    turn_add_tendril(writing, "call_1", "write",
                     "{\"text\":\"a </｜DSML｜parameter> b &lt;/｜DSML｜parameter> c\"}");
    prompt = chat_as_prompt(DIALECT_DEEPSEEK, NULL, NULL, &c);
    block = strstr(prompt, "</think>");
    EXPECT(block != NULL);
    EXPECT(read_tendrils(DIALECT_DEEPSEEK, block ? block + strlen("</think>") : "", NULL, &r));
    EXPECT(r.count == 1);
    if (r.count == 1) {
        EXPECT_SAME(r.tendrils[0].args_json,
                    "{\"text\":\"a </｜DSML｜parameter> b &lt;/｜DSML｜parameter> c\"}");
    }
    reaching_free(&r);
    free(prompt);
    chat_free(&c);
}

static void a_dsml41_block_is_read_into_tendrils(void) {
    const char *grown = "<｜DSML｜ calls>\n<｜DSML｜ invoke name=\"bash\">\n"
                        "<｜DSML｜ parameter name=\"command\" string=\"true\">ls -la</｜DSML｜ parameter>\n"
                        "</｜DSML｜ invoke>\n</｜DSML｜ calls>";
    reaching r;

    EXPECT(read_tendrils(DIALECT_DEEPSEEK41, grown, NULL, &r));
    EXPECT(r.count == 1 && r.words_end == 0);
    if (r.count == 1) {
        EXPECT_SAME(r.tendrils[0].name, "bash");
        EXPECT_SAME(r.tendrils[0].args_json, "{\"command\":\"ls -la\"}");
    }
    reaching_free(&r);
}

static void glm_arguments_follow_the_schema_types(void) {
    cJSON *tools = cJSON_Parse(WEATHER_TOOL);
    const char *grown = "sure\n\n<tool_call>weather<arg_key>city</arg_key><arg_value>2</arg_value>"
                        "<arg_key>days</arg_key><arg_value>2</arg_value></tool_call>";
    reaching r;

    EXPECT(read_tendrils(DIALECT_GLM, grown, tools, &r));
    EXPECT(r.count == 1 && r.words_end == strlen("sure") && r.tendrils_start == strlen("sure"));
    if (r.count == 1) EXPECT_SAME(r.tendrils[0].args_json, "{\"city\":\"2\",\"days\":2}");
    reaching_free(&r);
    cJSON_Delete(tools);
}

static void qwen_parameters_lose_their_framing_newlines(void) {
    const char *grown = "\n\n<tool_call>\n<function=weather>\n<parameter=city>\nOslo\n</parameter>\n"
                        "<parameter=days>\n2\n</parameter>\n</function>\n</tool_call>";
    reaching r;

    EXPECT(read_tendrils(DIALECT_QWEN, grown, NULL, &r));
    EXPECT(r.count == 1 && r.words_end == 0);
    if (r.count == 1) EXPECT_SAME(r.tendrils[0].args_json, "{\"city\":\"Oslo\",\"days\":2}");
    reaching_free(&r);
}

static void a_block_cut_off_is_only_words(void) {
    reaching r;
    EXPECT(!read_tendrils(DIALECT_DEEPSEEK, "x <｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"a\">", NULL, &r));
    EXPECT(r.count == 0 && r.tendrils == NULL);
    EXPECT(!read_tendrils(DIALECT_GLM, "<tool_call>a<arg_key>k</arg_key>", NULL, &r));
    EXPECT(!read_tendrils(DIALECT_QWEN, "plain words", NULL, &r));
}

static void words_are_shown_until_a_tendril_might_begin(void) {
    bool began = false;
    EXPECT(words_safe_to_show("hello wor", &began) == 9 && !began);
    EXPECT(words_safe_to_show("hello ", &began) == 5 && !began);
    EXPECT(words_safe_to_show("hello <｜DS", &began) == 5 && !began);
    EXPECT(words_safe_to_show("x <tool_cal", &began) == 1 && !began);
    EXPECT(words_safe_to_show("a <b", &began) == 4 && !began);
    EXPECT(words_safe_to_show("hello\n\n<｜DSML｜tool_calls>\n<x", &began) == 5 && began);
}

int main(void) {
    deepseek_offers_tools_after_the_system_words();
    deepseek_renders_a_graft_tendril_and_grows_on_from_its_answer();
    deepseek_renders_sampled_tendrils_as_they_were_grown();
    deepseek41_keeps_answers_in_one_user_turn();
    deepseek41_puts_tools_and_system_in_one_system_turn();
    glm_renders_tendrils_inline_and_answers_as_observations();
    glm_lists_tools_spaced_in_a_system_turn();
    qwen_renders_its_chatml_with_tool_responses();
    a_dsml_block_is_read_into_tendrils();
    a_dsml_closing_tag_inside_a_string_survives_the_round_trip();
    a_dsml41_block_is_read_into_tendrils();
    glm_arguments_follow_the_schema_types();
    qwen_parameters_lose_their_framing_newlines();
    a_block_cut_off_is_only_words();
    words_are_shown_until_a_tendril_might_begin();
    if (failures) {
        fprintf(stderr, "%d expectation%s failed\n", failures, failures == 1 ? "" : "s");
        return 1;
    }
    printf("chat: every expectation held\n");
    return 0;
}
