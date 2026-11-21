#ifndef DWARFSTAR_DS4_REFERENCE_H
#define DWARFSTAR_DS4_REFERENCE_H

typedef enum {
    RENDERED_AS_DEEPSEEK,
    RENDERED_AS_DEEPSEEK41,
    RENDERED_AS_GLM,
    RENDERED_AS_QWEN,
} reference_family;

char *ds4_server_prompt(reference_family family, const char *openai_messages,
                        const char *openai_tools);

char *ds4_server_calls(reference_family family, const char *grown);

#endif
