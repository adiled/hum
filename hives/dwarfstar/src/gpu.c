/* dwarfstar gpu: the hive-wide flock and the want directory beside it.
 * See gpu.h.
 */

#define _POSIX_C_SOURCE 200809L
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#else
#define _DEFAULT_SOURCE
#endif

#include "gpu.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define WANTS_SUFFIX ".wants"
#define POLL_MS 200
#define PROBE_TRIES 3
#define PROBE_RETRY_MS 5

struct dwarfstar_gpu {
    int fd;
    bool held;
    char wants[1100];
    char want[1200];
};

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000 + (int64_t)(ts.tv_nsec / 1000000);
}

static void sleep_ms(int ms) {
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    while (nanosleep(&ts, &ts) != 0 && errno == EINTR) {
    }
}

static int make_dirs(const char *path) {
    char buf[1100];
    char *p;
    snprintf(buf, sizeof buf, "%s", path);
    for (p = buf + 1; *p; p++) {
        if (*p != '/') continue;
        *p = '\0';
        if (mkdir(buf, 0700) != 0 && errno != EEXIST) return -1;
        *p = '/';
    }
    return mkdir(buf, 0700) == 0 || errno == EEXIST ? 0 : -1;
}

dwarfstar_gpu *gpu_open(const char *lock_path, char *err, size_t errlen) {
    dwarfstar_gpu *gpu;
    char dir[1100];
    char *slash;

    snprintf(dir, sizeof dir, "%s", lock_path);
    slash = strrchr(dir, '/');
    if (slash && slash != dir) {
        *slash = '\0';
        if (make_dirs(dir) != 0) {
            snprintf(err, errlen, "cannot make %s: %s", dir, strerror(errno));
            return NULL;
        }
    }
    gpu = (dwarfstar_gpu *)calloc(1, sizeof *gpu);
    if (!gpu) {
        snprintf(err, errlen, "out of memory");
        return NULL;
    }
    gpu->fd = open(lock_path, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
    if (gpu->fd < 0) {
        snprintf(err, errlen, "cannot open the GPU lock %s: %s", lock_path, strerror(errno));
        free(gpu);
        return NULL;
    }
    snprintf(gpu->wants, sizeof gpu->wants, "%s%s", lock_path, WANTS_SUFFIX);
    snprintf(gpu->want, sizeof gpu->want, "%s/%ld", gpu->wants, (long)getpid());
    if (make_dirs(gpu->wants) != 0) {
        snprintf(err, errlen, "cannot make %s: %s", gpu->wants, strerror(errno));
        close(gpu->fd);
        free(gpu);
        return NULL;
    }
    return gpu;
}

void gpu_close(dwarfstar_gpu *gpu) {
    if (!gpu) return;
    gpu_release(gpu);
    unlink(gpu->want);
    close(gpu->fd);
    free(gpu);
}

bool gpu_held(const dwarfstar_gpu *gpu) {
    return gpu->held;
}

static void file_want(dwarfstar_gpu *gpu) {
    int fd = open(gpu->want, O_WRONLY | O_CREAT | O_CLOEXEC, 0600);
    if (fd >= 0) close(fd);
}

int gpu_acquire(dwarfstar_gpu *gpu, int64_t wait_ms, gpu_stop_fn stop, void *user, char *err,
                size_t errlen) {
    int64_t deadline = now_ms() + wait_ms;
    bool wanted = false;

    if (gpu->held) return 0;
    for (;;) {
        if (flock(gpu->fd, LOCK_EX | LOCK_NB) == 0) {
            gpu->held = true;
            if (wanted) unlink(gpu->want);
            return 0;
        }
        if (errno != EWOULDBLOCK && errno != EINTR) {
            snprintf(err, errlen, "cannot lock the GPU: %s", strerror(errno));
            if (wanted) unlink(gpu->want);
            return -1;
        }
        if (!wanted) {
            file_want(gpu);
            wanted = true;
        }
        if (stop && stop(user)) {
            unlink(gpu->want);
            return 1;
        }
        if (now_ms() >= deadline) {
            snprintf(err, errlen, "another dwarfstar model held the GPU for %lld s",
                     (long long)(wait_ms / 1000));
            unlink(gpu->want);
            return -1;
        }
        sleep_ms(POLL_MS);
    }
}

void gpu_release(dwarfstar_gpu *gpu) {
    if (!gpu || !gpu->held) return;
    flock(gpu->fd, LOCK_UN);
    gpu->held = false;
}

gpu_state gpu_probe(dwarfstar_gpu *gpu) {
    int tries;
    if (gpu->held) return GPU_MINE;
    for (tries = 0; tries < PROBE_TRIES; tries++) {
        if (flock(gpu->fd, LOCK_EX | LOCK_NB) == 0) {
            flock(gpu->fd, LOCK_UN);
            return GPU_FREE;
        }
        sleep_ms(PROBE_RETRY_MS);
    }
    return GPU_OTHER;
}

int64_t gpu_oldest_want_ms(dwarfstar_gpu *gpu) {
    DIR *dir = opendir(gpu->wants);
    struct dirent *entry;
    int64_t now = now_ms();
    int64_t oldest = -1;
    long self = (long)getpid();

    if (!dir) return -1;
    while ((entry = readdir(dir)) != NULL) {
        char path[1400];
        char *end = NULL;
        long pid = strtol(entry->d_name, &end, 10);
        struct stat st;

        if (pid <= 0 || !end || *end != '\0' || pid == self) continue;
        snprintf(path, sizeof path, "%s/%s", gpu->wants, entry->d_name);
        if (kill((pid_t)pid, 0) != 0 && errno == ESRCH) {
            unlink(path);
            continue;
        }
        if (stat(path, &st) != 0) continue;
        {
            int64_t age = now - (int64_t)st.st_mtime * 1000;
            if (age < 0) age = 0;
            if (age > oldest) oldest = age;
        }
    }
    closedir(dir);
    return oldest;
}
