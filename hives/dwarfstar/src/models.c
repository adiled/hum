/* dwarfstar models: scanning DWARFSTAR_DIR for GGUFs and naming them.
 * See models.h.
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif

#include "models.h"

#include <dirent.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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
           has(name, "Layers00-30") || has(name, "Layers-31-output") || has(name, "-of-");
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

static int already_have(const ds4_models *models, const char *id) {
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

static void add_quantised(ds4_models *models, const char *family, const char *name,
                          const char *path) {
    char quant[16];
    char id[128];
    if (!quant_suffix(name, quant, sizeof quant)) return;
    snprintf(id, sizeof id, "%s%s", family, quant);
    add(models, id, path);
}

int ds4_models_scan(ds4_models *models, const char *dir) {
    char default_path[1024];
    struct stat st;
    int have_default;
    int pass;

    memset(models, 0, sizeof *models);
    if (!dir || !dir[0]) return -1;

    snprintf(default_path, sizeof default_path, "%s/ds4flash.gguf", dir);
    have_default = stat(default_path, &st) == 0;
    if (have_default) snprintf(models->default_path, sizeof models->default_path, "%s", default_path);

    for (pass = 0; pass < 2; pass++) {
        char scan_dir[1024];
        DIR *handle;
        struct dirent *entry;

        snprintf(scan_dir, sizeof scan_dir, pass == 0 ? "%s" : "%s/gguf", dir);
        handle = opendir(scan_dir);
        if (!handle) continue;
        while ((entry = readdir(handle)) != NULL) {
            char path[1200];
            char resolved[PATH_MAX];
            const char *family;

            if (!ends_with(entry->d_name, ".gguf")) continue;
            if (is_support_file(entry->d_name)) continue;
            snprintf(path, sizeof path, "%s/%s", scan_dir, entry->d_name);
            if (stat(path, &st) != 0) continue;

            family = family_of(entry->d_name);
            if (!family) {
                const char *base = realpath(path, resolved) ? strrchr(resolved, '/') : NULL;
                if (!base || is_support_file(base)) continue;
                family = family_of(base);
                if (!family) continue;
                add_quantised(models, family, entry->d_name, path);
                add(models, family, path);
                continue;
            }
            add_quantised(models, family, entry->d_name, path);
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

int ds4_models_pin(ds4_models *models, const char *path, const char *ids) {
    char resolved[PATH_MAX];
    const char *name;
    const char *family;
    struct stat st;

    memset(models, 0, sizeof *models);
    if (!path || !path[0] || stat(path, &st) != 0) return -1;
    snprintf(models->default_path, sizeof models->default_path, "%s", path);
    if (ids && ids[0]) {
        const char *at = ids;
        while (*at) {
            size_t n = strcspn(at, ",");
            char id[128];
            if (n > 0 && n < sizeof id) {
                memcpy(id, at, n);
                id[n] = '\0';
                add(models, id, path);
            }
            at += n + (at[n] == ',' ? 1 : 0);
        }
        return models->count;
    }
    name = strrchr(path, '/') ? strrchr(path, '/') + 1 : path;
    family = family_of(name);
    if (!family && realpath(path, resolved)) {
        name = strrchr(resolved, '/') ? strrchr(resolved, '/') + 1 : resolved;
        family = family_of(name);
    }
    if (!family) return 0;
    add(models, family, path);
    add_quantised(models, family, name, path);
    return models->count;
}

const char *ds4_models_path(const ds4_models *models, const char *id) {
    int i;
    if (!id || !id[0]) {
        if (models->default_path[0]) return models->default_path;
        return models->count > 0 ? models->entries[0].path : NULL;
    }
    for (i = 0; i < models->count; i++) {
        if (strcmp(models->entries[i].id, id) == 0) return models->entries[i].path;
    }
    return NULL;
}

cJSON *ds4_models_ids(const ds4_models *models) {
    cJSON *ids = cJSON_CreateArray();
    int i;
    if (!ids) return NULL;
    for (i = 0; i < models->count; i++) {
        cJSON_AddItemToArray(ids, cJSON_CreateString(models->entries[i].id));
    }
    return ids;
}
