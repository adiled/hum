/* dwarfstar models: the DwarfStar models on this machine, by id.
 *
 * The bee looks in DWARFSTAR_DIR and in the gguf/ directory under it,
 * follows the symlinks ds4 keeps for its current defaults, and gives each
 * model it can serve an id humd routes on: the family (deepseek-v4-flash,
 * deepseek-v4.1-flash, glm-5.3-flash, glm-5.3, glm-5.2, qwen3.8-flash-next,
 * deepseek-v4-pro), and the family with its quantisation (-q4, -q2,
 * -q2-q4, -mxfp4, -fp8) when the file name says which. Vision encoders,
 * DSpark support files and split layer pieces are not models. A prompt
 * that names no model gets ds4's own default, ds4flash.gguf, when there
 * is one, and otherwise the first model found.
 *
 * A unit that pins its bee to one model uses ds4_models_pin instead: the
 * one file, answering to the ids given (comma separated), or to the ids
 * its file name earns when none are given.
 */

#ifndef DWARFSTAR_MODELS_H
#define DWARFSTAR_MODELS_H

#include <cjson/cJSON.h>

#define DS4_MAX_MODELS 32

typedef struct {
    char id[128];
    char path[1024];
} ds4_model_entry;

typedef struct {
    ds4_model_entry entries[DS4_MAX_MODELS];
    int count;
    char default_path[1024];
} ds4_models;

int ds4_models_scan(ds4_models *models, const char *dir);
int ds4_models_pin(ds4_models *models, const char *path, const char *ids);
const char *ds4_models_path(const ds4_models *models, const char *id);
cJSON *ds4_models_ids(const ds4_models *models);

#endif
