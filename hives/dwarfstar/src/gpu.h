#ifndef DWARFSTAR_GPU_H
#define DWARFSTAR_GPU_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum {
    GPU_FREE,
    GPU_MINE,
    GPU_OTHER
} gpu_state;

typedef struct dwarfstar_gpu dwarfstar_gpu;

typedef bool (*gpu_stop_fn)(void *user);

dwarfstar_gpu *gpu_open(const char *lock_path, char *err, size_t errlen);
void gpu_close(dwarfstar_gpu *gpu);

int gpu_acquire(dwarfstar_gpu *gpu, int64_t wait_ms, gpu_stop_fn stop, void *user, char *err,
                size_t errlen);
void gpu_release(dwarfstar_gpu *gpu);
bool gpu_held(const dwarfstar_gpu *gpu);
gpu_state gpu_probe(dwarfstar_gpu *gpu);
int64_t gpu_oldest_want_ms(dwarfstar_gpu *gpu);

#endif
