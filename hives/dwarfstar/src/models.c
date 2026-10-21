/* dwarfstar models — find the DwarfStar models on this machine.
 *
 * ds4 keeps its GGUFs wherever its download script put them. The bee
 * looks there, works out which files are models it can actually serve,
 * and gives each one an id that humd can route on.
 *
 * The ids are the families ds4 knows: deepseek-v4-flash, deepseek-v4.1-flash,
 * glm-5.3-flash, glm-5.2, qwen3.8-flash-next, deepseek-v4-pro. A family
 * with several files on disk also gets a suffixed id per quantisation
 * (deepseek-v4-flash-q4, -q2, -mxfp4), so a prompt can name the exact
 * artifact it wants.
 */

#include "models.h"

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>
#include <stdlib.h>
#include <sys/stat.h>

static int has(const char *haystack, const char *needle) {
    return haystack && needle && strstr(haystack, needle) != NULL;
}

static int ends_with(const char *text, const char *suffix) {
    size_t tl = strlen(text);
    size_t sl = strlen(suffix);
    return tl >= sl && strcmp(text + tl - sl, suffix) == 0;
}

static int is_support_file(const char *name) {
    return has(name, "Vision-Encoder") || has(name, "DSpark-support") ||
           has(name, "Layers00-30") || has(name, "Layers-31-output") ||
           has(name, "-of-");
}

static int quant_suffix(const char *name, char *out, size_t cap) {
    int q4 = has(name, "Q4K") || has(name, "Q4_K") || has(name, "Q4");
    int q2 = has(name, "Q2_K") || has(name, "IQ2_XXS") || has(name, "Q2K") || has(name, "Q2");
    if (q4 && q2) {
        snprintf(out, cap, "-q2-q4");
        return 1;
    }
    if (has(name, "MXFP4")) {
        snprintf(out, cap, "-mxfp4");
        return 1;
    }
    if (q4) {
        snprintf(out, cap, "-q4");
        return 1;
    }
    if (q2) {
        snprintf(out, cap, "-q2");
        return 1;
    }
    if (has(name, "FP8")) {
        snprintf(out, cap, "-fp8");
        return 1;
    }
    return 0;
}

static const char *family_of(const char *name) {
    if (has(name, "DeepSeek-V4.1-Flash")) return "deepseek-v4.1-flash";
    if (has(name, "DeepSeek-V4-Flash")) return "deepseek-v4-flash";
    if (has(name, "DeepSeek-V4-Pro")) return "deepseek-v4-pro";
    if (has(name, "GLM-5.3-Flash")) return "glm-5.3-flash";
    if (has(name, "GLM-5.3")) return "glm-5.3";
    if (has(name, "GLM-5.2")) return "glm-5.2";
    if (has(name, "Qwen")) return "qwen3.8-flash-next";
    return NULL;
}

static int already_have(ds4_models *models, const char *id) {
    int i;
    for (i = 0; i < models->count; i++) {
        if (strcmp(models->entries[i].id, id) == 0) return 1;
    }
    return 0;
}

static void add(ds4_models *models, const char *id, const char *path) {
    if (models->count >= DS4_MAX_MODELS) return;
    if (already_have(models, id)) return;
    snprintf(models->entries[models->count].id, sizeof models->entries[0].id, "%s", id);
    snprintf(models->entries[models->count].path, sizeof models->entries[0].path, "%s", path);
    models->count++;
}

int ds4_models_scan(ds4_models *models, const char *dir) {
    DIR *handle;
    struct dirent *entry;
    char default_path[1024];
    int have_default = 0;
    int pass;

    memset(models, 0, sizeof *models);
    if (!dir || !dir[0]) return -1;

    snprintf(default_path, sizeof default_path, "%s/ds4flash.gguf", dir);
    {
        struct stat st;
        have_default = stat(default_path, &st) == 0;
    }

    for (pass = 0; pass < 2; pass++) {
        char scan_dir[1024];
        if (pass == 0) {
            snprintf(scan_dir, sizeof scan_dir, "%s", dir);
        } else {
            snprintf(scan_dir, sizeof scan_dir, "%s/gguf", dir);
        }
        handle = opendir(scan_dir);
        if (!handle) continue;
        while ((entry = readdir(handle)) != NULL) {
            char path[1200];
            const char *family;
            char quant[16];
            struct stat st;
            char resolved[1200];

            if (!ends_with(entry->d_name, ".gguf")) continue;
            if (is_support_file(entry->d_name)) continue;

            snprintf(path, sizeof path, "%s/%s", scan_dir, entry->d_name);
            if (stat(path, &st) != 0) continue;

            family = family_of(entry->d_name);
            if (!family) {
                const char *target = realpath(path, resolved) ? resolved : NULL;
                const char *base = target ? strrchr(target, '/') : NULL;
                if (!base) continue;
                if (is_support_file(base)) continue;
                family = family_of(base);
                if (!family) continue;
                if (quant_suffix(entry->d_name, quant, sizeof quant)) {
                    char id[128];
                    snprintf(id, sizeof id, "%s%s", family, quant);
                    add(models, id, path);
                }
                add(models, family, path);
                continue;
            }
            if (quant_suffix(entry->d_name, quant, sizeof quant)) {
                char id[128];
                snprintf(id, sizeof id, "%s%s", family, quant);
                add(models, id, path);
            }
            if (have_default && strcmp(path, default_path) == 0) {
                add(models, family, path);
            } else if (!already_have(models, family)) {
                add(models, family, path);
            }
        }
        closedir(handle);
    }
    return models->count;
}

const char *ds4_models_path(const ds4_models *models, const char *id) {
    int i;
    if (!id || !id[0]) {
        for (i = 0; i < models->count; i++) {
            if (strchr(models->entries[i].id, '-') == NULL) return models->entries[i].path;
        }
        return models->count > 0 ? models->entries[0].path : NULL;
    }
    for (i = 0; i < models->count; i++) {
        if (strcmp(models->entries[i].id, id) == 0) return models->entries[i].path;
    }
    return NULL;
}

void ds4_models_json(const ds4_models *models, thrum_buf *buf) {
    int i;
    thrum_buf_append_str(buf, "[");
    for (i = 0; i < models->count; i++) {
        if (i > 0) thrum_buf_append_str(buf, ",");
        thrum_buf_append_json_string(buf, models->entries[i].id);
    }
    thrum_buf_append_str(buf, "]");
}
