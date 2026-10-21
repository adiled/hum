/* dwarfstar shaders — point the engine at its Metal sources.
 *
 * ds4 compiles its Metal kernels at runtime from .metal files it finds
 * next to the binary (or wherever DS4_METAL_*_SOURCE points). The hive
 * ships those files beside itself, so all we do here is set the override
 * for each kernel to an absolute path. No guessing about the working
 * directory, no surprises when humd starts the bee from /.
 */

#include "shaders.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "ds4.h"

static const struct {
    const char *env;
    const char *file;
} kShaders[] = {
    {"DS4_METAL_FLASH_ATTN_SOURCE", "flash_attn.metal"},
    {"DS4_METAL_DENSE_SOURCE", "dense.metal"},
    {"DS4_METAL_GLM53_BF16_SOURCE", "glm53_bf16.metal"},
    {"DS4_METAL_GLM53_VISION_SOURCE", "glm53_vision.metal"},
    {"DS4_METAL_DEEPSEEK4_VISION_SOURCE", "deepseek4_vision.metal"},
    {"DS4_METAL_GLM53_KDA_SOURCE", "glm53_kda.metal"},
    {"DS4_METAL_MOE_SOURCE", "moe.metal"},
    {"DS4_METAL_DSV4_HC_SOURCE", "dsv4_hc.metal"},
    {"DS4_METAL_UNARY_SOURCE", "unary.metal"},
    {"DS4_METAL_DSV4_KV_SOURCE", "dsv4_kv.metal"},
    {"DS4_METAL_DSV41_SOURCE", "dsv41.metal"},
    {"DS4_METAL_DSV4_ROPE_SOURCE", "dsv4_rope.metal"},
    {"DS4_METAL_DSV4_MISC_SOURCE", "dsv4_misc.metal"},
    {"DS4_METAL_ARGSORT_SOURCE", "argsort.metal"},
    {"DS4_METAL_CPY_SOURCE", "cpy.metal"},
    {"DS4_METAL_CONCAT_SOURCE", "concat.metal"},
    {"DS4_METAL_GET_ROWS_SOURCE", "get_rows.metal"},
    {"DS4_METAL_SUM_ROWS_SOURCE", "sum_rows.metal"},
    {"DS4_METAL_SOFTMAX_SOURCE", "softmax.metal"},
    {"DS4_METAL_REPEAT_SOURCE", "repeat.metal"},
    {"DS4_METAL_GLU_SOURCE", "glu.metal"},
    {"DS4_METAL_NORM_SOURCE", "norm.metal"},
    {"DS4_METAL_BIN_SOURCE", "bin.metal"},
    {"DS4_METAL_SET_ROWS_SOURCE", "set_rows.metal"},
    {"DS4_METAL_QWEN4_SOURCE", "qwen4.metal"},
    {"DS4_METAL_QWEN4_VISION_SOURCE", "qwen4_vision.metal"},
};

int dwarfstar_shaders_point_at(const char *dir) {
    size_t i;
    size_t count = sizeof kShaders / sizeof kShaders[0];
    if (!dir || !dir[0]) return -1;
    for (i = 0; i < count; i++) {
        char path[2048];
        snprintf(path, sizeof path, "%s/%s", dir, kShaders[i].file);
        if (access(path, R_OK) != 0) {
            fprintf(stderr, "dwarfstar: metal source missing: %s\n", path);
            return -1;
        }
        setenv(kShaders[i].env, path, 1);
    }
    return 0;
}
