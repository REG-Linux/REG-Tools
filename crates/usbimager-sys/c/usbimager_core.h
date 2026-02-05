#ifndef RL_USBIMAGER_CORE_H
#define RL_USBIMAGER_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    char *id;
    char *label;
    uint64_t size_bytes;
    int is_removable;
} rl_device;

typedef void (*rl_progress_cb)(void *user, uint64_t done, uint64_t total, const char *message);
typedef void (*rl_error_cb)(void *user, const char *message);

typedef struct rl_job rl_job;

int rl_list_devices(int show_all, rl_device **out_devices, size_t *out_len);
void rl_free_devices(rl_device *devices, size_t len);

rl_job *rl_write_image_zst(const char *image_path, const char *device_id, int verify,
    rl_progress_cb progress_cb, rl_error_cb error_cb, void *user);
int rl_cancel(rl_job *job);
int rl_wait(rl_job *job);
void rl_free(rl_job *job);

const char *rl_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
