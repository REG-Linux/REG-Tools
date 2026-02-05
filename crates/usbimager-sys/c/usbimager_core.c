#include "usbimager_core.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "disks.h"
#include "lang.h"
#include "stream.h"

#ifdef __linux__
#include <sys/stat.h>
#endif

char **lang = NULL;
char *main_errorMessage = NULL;
extern char *dict[NUMLANGS][NUMTEXTS + 1];

static char rl_last_error_buf[256];

static void rl_set_last_error(const char *msg)
{
    if(!msg) msg = "Unknown error";
    snprintf(rl_last_error_buf, sizeof(rl_last_error_buf), "%s", msg);
    main_errorMessage = rl_last_error_buf;
}

const char *rl_last_error(void)
{
    return rl_last_error_buf[0] ? rl_last_error_buf : NULL;
}

void main_getErrorMessage(void)
{
    rl_set_last_error(strerror(errno));
}

void main_onProgress(void *data)
{
    (void)data;
}

static rl_device *g_devices = NULL;
static size_t g_device_cap = 0;
static size_t g_device_count = 0;
static int g_show_all = 0;

extern char disks_devs[DISKS_MAX][32];

static int rl_is_removable(const char *name)
{
#ifdef __linux__
    char path[PATH_MAX];
    char buf[8];
    FILE *f;
    if(!name || !*name) return 0;
    snprintf(path, sizeof(path), "/sys/block/%s/removable", name);
    f = fopen(path, "r");
    if(!f) return 0;
    if(!fgets(buf, sizeof(buf), f)) { fclose(f); return 0; }
    fclose(f);
    return buf[0] == '1';
#else
    (void)name;
    return 0;
#endif
}

void main_addToCombobox(char *option)
{
    char name[64];
    char idbuf[128];
    const char *space;
    size_t len;
    rl_device *dev;

    if(!g_devices || g_device_count >= g_device_cap || !option) return;

    space = strchr(option, ' ');
    len = space ? (size_t)(space - option) : strlen(option);
    if(len >= sizeof(name)) len = sizeof(name) - 1;
    memcpy(name, option, len);
    name[len] = 0;

    dev = &g_devices[g_device_count];
    memset(dev, 0, sizeof(*dev));
    dev->label = strdup(option);

    if(name[0] == '/') {
        dev->id = strdup(name);
    } else if(!strncmp(name, "sdT", 3)) {
        snprintf(idbuf, sizeof(idbuf), "%s", name);
        dev->id = strdup(idbuf);
    } else {
        snprintf(idbuf, sizeof(idbuf), "/dev/%s", name);
        dev->id = strdup(idbuf);
    }

    dev->size_bytes = disks_capacity[g_device_count];
    dev->is_removable = rl_is_removable(name);

    g_device_count++;
}

int rl_list_devices(int show_all, rl_device **out_devices, size_t *out_len)
{
    if(!out_devices || !out_len) return -1;

    if(!lang) lang = &dict[0][1];

    g_show_all = show_all ? 1 : 0;
    disks_all = g_show_all;
    disks_serial = 0;

    g_device_cap = DISKS_MAX;
    g_device_count = 0;
    g_devices = (rl_device*)calloc(g_device_cap, sizeof(rl_device));
    if(!g_devices) {
        rl_set_last_error("Out of memory");
        return -1;
    }

    disks_refreshlist();

    *out_devices = g_devices;
    *out_len = g_device_count;

    g_devices = NULL;
    g_device_cap = 0;
    g_device_count = 0;

    return 0;
}

void rl_free_devices(rl_device *devices, size_t len)
{
    size_t i;
    if(!devices) return;
    for(i = 0; i < len; i++) {
        free(devices[i].id);
        free(devices[i].label);
    }
    free(devices);
}

struct rl_job {
    pthread_t thread;
    int cancel;
    int done;
    int result;
    int verify;
    rl_progress_cb progress_cb;
    rl_error_cb error_cb;
    void *user;
    char image_path[PATH_MAX];
    char device_id[128];
    char error[256];
};

static void rl_emit_progress(struct rl_job *job, stream_t *ctx, int done)
{
    char status[128];
    if(!job || !job->progress_cb) return;
    memset(status, 0, sizeof(status));
    stream_status(ctx, status, done);
    job->progress_cb(job->user, ctx->readSize, ctx->fileSize, status);
}

static void rl_set_job_error(struct rl_job *job, const char *msg)
{
    if(!job) return;
    rl_set_last_error(msg);
    snprintf(job->error, sizeof(job->error), "%s", rl_last_error_buf);
    if(job->error_cb) job->error_cb(job->user, job->error);
}

static int rl_find_target_index(const char *device_id)
{
    int i;
    const char *name = device_id;

    if(!device_id || !*device_id) return -1;
    if(!lang) lang = &dict[0][1];

    if(!strncmp(device_id, "/dev/", 5)) name = device_id + 5;

    disks_all = 1;
    disks_serial = 0;
    disks_refreshlist();

    for(i = 0; i < DISKS_MAX; i++) {
        if(disks_targets[i] == -1) continue;
        if(!strncmp(disks_devs[i], name, sizeof(disks_devs[i]))) return i;
    }
    return -1;
}

static const char *rl_stream_error(int code)
{
    switch(code) {
        case 2: return lang[L_ENCZIPERR];
        case 3: return lang[L_CMPZIPERR];
        case 4: return lang[L_CMPERR];
        default: return lang[L_SRCERR];
    }
}

static const char *rl_open_error(long code)
{
    if(code == -1) return lang[L_TRGERR];
    if(code == -2) return lang[L_UMOUNTERR];
    if(code == -4) return lang[L_COMMERR];
    return lang[L_OPENTRGERR];
}

static void *rl_writer_thread(void *arg)
{
    struct rl_job *job = (struct rl_job *)arg;
    stream_t ctx;
    int dst;
    int numberOfBytesRead;
    int numberOfBytesWritten;
    int numberOfBytesVerify;
    int needWrite;
    int targetId;

    memset(&ctx, 0, sizeof(ctx));

    if(!lang) lang = &dict[0][1];

    int open_res = stream_open(&ctx, job->image_path, 0);
    if(open_res) {
        rl_set_job_error(job, rl_stream_error(open_res));
        job->result = 1;
        job->done = 1;
        return NULL;
    }

    targetId = rl_find_target_index(job->device_id);
    if(targetId < 0) {
        rl_set_job_error(job, "Device not found");
        stream_close(&ctx);
        job->result = 1;
        job->done = 1;
        return NULL;
    }

    dst = (int)((long int)disks_open(targetId, ctx.fileSize));
    if(dst <= 0) {
        rl_set_job_error(job, rl_open_error((long)dst));
        stream_close(&ctx);
        job->result = 1;
        job->done = 1;
        return NULL;
    }

    rl_emit_progress(job, &ctx, 0);

    while(!job->cancel) {
        numberOfBytesRead = stream_read(&ctx);
        if(numberOfBytesRead < 0) {
            rl_set_job_error(job, lang[L_RDSRCERR]);
            job->result = 1;
            break;
        }
        if(numberOfBytesRead == 0) {
            if(!ctx.fileSize) ctx.fileSize = ctx.readSize;
            break;
        }
        errno = 0;
        needWrite = 1;
        if(!force) {
            numberOfBytesVerify = (int)read(dst, ctx.verifyBuf, numberOfBytesRead);
            if(numberOfBytesVerify == numberOfBytesRead &&
                !memcmp(ctx.buffer, ctx.verifyBuf, numberOfBytesRead)) {
                needWrite = 0;
                rl_emit_progress(job, &ctx, 0);
            } else {
                lseek(dst, -((off_t)numberOfBytesVerify), SEEK_CUR);
            }
        }
        if(needWrite) {
            numberOfBytesWritten = (int)write(dst, ctx.buffer, numberOfBytesRead);
            if(numberOfBytesWritten == numberOfBytesRead) {
                if(job->verify) {
                    lseek(dst, -((off_t)numberOfBytesWritten), SEEK_CUR);
                    numberOfBytesVerify = (int)read(dst, ctx.verifyBuf, numberOfBytesWritten);
                    if(numberOfBytesVerify != numberOfBytesWritten ||
                        memcmp(ctx.buffer, ctx.verifyBuf, numberOfBytesWritten)) {
                        rl_set_job_error(job, lang[L_VRFYERR]);
                        job->result = 1;
                        break;
                    }
                }
                rl_emit_progress(job, &ctx, 0);
            } else {
                if(errno) main_getErrorMessage();
                rl_set_job_error(job, lang[L_WRTRGERR]);
                job->result = 1;
                break;
            }
        }
    }

    if(job->cancel && job->result == 0) {
        rl_set_job_error(job, "Cancelled");
        job->result = 2;
    }

    stream_status(&ctx, job->error, 1);
    rl_emit_progress(job, &ctx, 1);

    disks_close((void *)((long int)dst));
    stream_close(&ctx);

    job->done = 1;
    return NULL;
}

rl_job *rl_write_image_zst(const char *image_path, const char *device_id, int verify,
    rl_progress_cb progress_cb, rl_error_cb error_cb, void *user)
{
    struct rl_job *job;

    if(!image_path || !device_id) {
        rl_set_last_error("Invalid arguments");
        return NULL;
    }

    if(!lang) lang = &dict[0][1];

    job = (struct rl_job*)calloc(1, sizeof(struct rl_job));
    if(!job) {
        rl_set_last_error("Out of memory");
        return NULL;
    }

    snprintf(job->image_path, sizeof(job->image_path), "%s", image_path);
    snprintf(job->device_id, sizeof(job->device_id), "%s", device_id);
    job->verify = verify ? 1 : 0;
    job->progress_cb = progress_cb;
    job->error_cb = error_cb;
    job->user = user;
    job->result = 0;

    if(pthread_create(&job->thread, NULL, rl_writer_thread, job) != 0) {
        rl_set_last_error("Failed to start writer thread");
        free(job);
        return NULL;
    }

    return (rl_job*)job;
}

int rl_cancel(rl_job *job)
{
    if(!job) return -1;
    job->cancel = 1;
    return 0;
}

int rl_wait(rl_job *job)
{
    if(!job) return -1;
    pthread_join(job->thread, NULL);
    return job->result;
}

void rl_free(rl_job *job)
{
    if(!job) return;
    if(!job->done) {
        job->cancel = 1;
        pthread_join(job->thread, NULL);
    }
    free(job);
}
