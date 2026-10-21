/* dwarfstar models — find the DwarfStar models on this machine. */

#ifndef DWARFSTAR_MODELS_H
#define DWARFSTAR_MODELS_H

#include <stddef.h>

#include "thrum/json.h"

#define DS4_MAX_MODELS 32

typedef struct {
    char id[128];
    char path[1024];
} ds4_model_entry;

typedef struct {
    ds4_model_entry entries[DS4_MAX_MODELS];
    int count;
} ds4_models;

int ds4_models_scan(ds4_models *models, const char *dir);
int ds4_models_pin(ds4_models *models, const char *path);
const char *ds4_models_path(const ds4_models *models, const char *id);
void ds4_models_json(const ds4_models *models, thrum_buf *buf);

#endif /* DWARFSTAR_MODELS_H */
