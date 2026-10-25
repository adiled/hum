/* dwarfstar gpu: one model on the GPU across every bee of the hive.
 *
 * Each model is its own unit and its own bee, and a bee loads its model
 * only when a prompt needs it. ds4 refuses a second process on the GPU by
 * exiting, so the bees take a hive-wide lock first: a flock on
 * DWARFSTAR_GPU_LOCK, held for as long as the engine is loaded. A bee that
 * needs the GPU while another holds it files a want, an empty file named
 * by its pid in the directory beside the lock, and waits. The holder hands
 * the GPU over when it is idle and someone wants it, after its current
 * bloom when a want has waited a minute, and on its own after
 * DWARFSTAR_IDLE_MS idle. A want left by a process that died is removed by
 * whoever sees it. A probe holds the lock for an instant, so it tries a
 * few times before it calls the GPU another bee's.
 */

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
