#include "usbimager_core.h"

#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#include <io.h>
#include <fcntl.h>
#else
#include <fcntl.h>
#include <unistd.h>
#include <pthread.h>
#endif

#include "disks.h"
#include "lang.h"
#include "stream.h"

#ifdef __linux__
#include <sys/stat.h>
#endif

extern char *dict[NUMLANGS][NUMTEXTS + 1];

#ifdef _WIN32
wchar_t **lang = NULL;
#else
char **lang = NULL;
#endif

char *main_errorMessage = NULL;

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
#ifdef _WIN32
    DWORD err = GetLastError();
    if(!err) {
        rl_set_last_error("Unknown error");
        return;
    }
    FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
        NULL, err, MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT),
        rl_last_error_buf, (DWORD)sizeof(rl_last_error_buf), NULL);
#else
    rl_set_last_error(strerror(errno));
#endif
}

void main_onProgress(void *data)
{
    (void)data;
}

static rl_device *g_devices = NULL;
static size_t g_device_cap = 0;
static size_t g_device_count = 0;
static int g_show_all = 0;

#ifdef __linux__
extern char disks_devs[DISKS_MAX][32];
#endif
#ifdef __APPLE__
extern char disks_serials[DISKS_MAX][64];
#endif

#ifdef _WIN32
static char *rl_wide_to_utf8(const wchar_t *wstr)
{
    int len;
    char *buf;

    if(!wstr) return strdup("");
    len = WideCharToMultiByte(CP_UTF8, 0, wstr, -1, NULL, 0, NULL, NULL);
    if(len <= 0) return strdup("");
    buf = (char*)malloc((size_t)len);
    if(!buf) return strdup("");
    WideCharToMultiByte(CP_UTF8, 0, wstr, -1, buf, len, NULL, NULL);
    return buf;
}

static void rl_init_lang(void)
{
    int j;
    if(lang) return;
    lang = (wchar_t**)calloc(NUMTEXTS, sizeof(wchar_t*));
    if(!lang) return;
    for(j = 0; j < NUMTEXTS; j++) {
        const char *src = dict[0][j + 1];
        int len = MultiByteToWideChar(CP_UTF8, 0, src, -1, NULL, 0);
        if(len <= 0) continue;
        lang[j] = (wchar_t*)calloc((size_t)len, sizeof(wchar_t));
        if(!lang[j]) continue;
        MultiByteToWideChar(CP_UTF8, 0, src, -1, lang[j], len);
    }
}

static const char *rl_lang_utf8(int idx)
{
    static char buf[256];
    int len;
    if(idx < 0 || idx >= NUMTEXTS) return "Error";
    rl_init_lang();
    if(!lang || !lang[idx]) return "Error";
    len = WideCharToMultiByte(CP_UTF8, 0, lang[idx], -1, buf, (int)sizeof(buf), NULL, NULL);
    if(len <= 0) return "Error";
    return buf;
}
#else
static void rl_init_lang(void)
{
    if(!lang) lang = &dict[0][1];
}

static const char *rl_lang_utf8(int idx)
{
    rl_init_lang();
    if(!lang || idx < 0 || idx >= NUMTEXTS) return "Error";
    return lang[idx];
}
#endif

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
    int index;
    const char *removable_name = NULL;

    if(!g_devices || g_device_count >= g_device_cap || !option) return;

    index = (int)g_device_count;

#ifdef _WIN32
    {
        wchar_t *wopt = (wchar_t*)option;
        char *utf8 = rl_wide_to_utf8(wopt);
        dev = &g_devices[g_device_count];
        memset(dev, 0, sizeof(*dev));
        dev->label = utf8;

        if(index >= 0 && index < DISKS_MAX) {
            int target = disks_targets[index];
            if(target >= 1024) {
                snprintf(idbuf, sizeof(idbuf), "\\\\.\\COM%d", target - 1024);
            } else {
                snprintf(idbuf, sizeof(idbuf), "\\\\.\\PhysicalDrive%d", target);
            }
            dev->id = strdup(idbuf);
        } else {
            dev->id = strdup("");
        }
    }
#else
    space = strchr(option, ' ');
    len = space ? (size_t)(space - option) : strlen(option);
    if(len >= sizeof(name)) len = sizeof(name) - 1;
    memcpy(name, option, len);
    name[len] = 0;
    removable_name = name;

    dev = &g_devices[g_device_count];
    memset(dev, 0, sizeof(*dev));
    dev->label = strdup(option);

#if defined(__APPLE__)
    if(index >= 0 && index < DISKS_MAX) {
        int target = disks_targets[index];
        if(target >= 1024) {
            snprintf(idbuf, sizeof(idbuf), "/dev/%s", disks_serials[target - 1024]);
        } else {
            snprintf(idbuf, sizeof(idbuf), "/dev/disk%d", target);
        }
        dev->id = strdup(idbuf);
    } else {
        dev->id = strdup("");
    }
#else
    if(name[0] == '/') {
        dev->id = strdup(name);
    } else if(!strncmp(name, "sdT", 3)) {
        snprintf(idbuf, sizeof(idbuf), "%s", name);
        dev->id = strdup(idbuf);
    } else {
        snprintf(idbuf, sizeof(idbuf), "/dev/%s", name);
        dev->id = strdup(idbuf);
    }
#endif
#endif

    dev->size_bytes = disks_capacity[g_device_count];
    if(g_show_all) {
#ifdef __linux__
        dev->is_removable = removable_name ? rl_is_removable(removable_name) : 0;
#else
        dev->is_removable = 0;
#endif
    } else {
        dev->is_removable = 1;
    }

    g_device_count++;
}

int rl_list_devices(int show_all, rl_device **out_devices, size_t *out_len)
{
    if(!out_devices || !out_len) return -1;

    rl_init_lang();

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
#ifdef _WIN32
    HANDLE thread;
#else
    pthread_t thread;
#endif
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
    if(!job || !job->progress_cb) return;
#ifdef _WIN32
    wchar_t status_w[128];
    char status[256];
    memset(status_w, 0, sizeof(status_w));
    memset(status, 0, sizeof(status));
    stream_status(ctx, (char*)status_w, done);
    WideCharToMultiByte(CP_UTF8, 0, status_w, -1, status, (int)sizeof(status), NULL, NULL);
    job->progress_cb(job->user, ctx->readSize, ctx->fileSize, status);
#else
    char status[128];
    memset(status, 0, sizeof(status));
    stream_status(ctx, status, done);
    job->progress_cb(job->user, ctx->readSize, ctx->fileSize, status);
#endif
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

    if(!device_id || !*device_id) return -1;
    rl_init_lang();

    disks_all = 1;
    disks_serial = 0;
    disks_refreshlist();

#ifdef _WIN32
    if(!strncmp(device_id, "\\\\.\\PhysicalDrive", 17)) {
        int num = atoi(device_id + 17);
        for(i = 0; i < DISKS_MAX; i++) {
            if(disks_targets[i] == num) return i;
        }
    }
    if(!strncmp(device_id, "\\\\.\\COM", 8)) {
        int num = atoi(device_id + 8);
        for(i = 0; i < DISKS_MAX; i++) {
            if(disks_targets[i] == 1024 + num) return i;
        }
    }
    if(!strncmp(device_id, "COM", 3)) {
        int num = atoi(device_id + 3);
        for(i = 0; i < DISKS_MAX; i++) {
            if(disks_targets[i] == 1024 + num) return i;
        }
    }
#elif defined(__APPLE__)
    {
        const char *name = device_id;
        if(!strncmp(device_id, "/dev/", 5)) name = device_id + 5;
        if(!strncmp(name, "disk", 4) || !strncmp(name, "rdisk", 5)) {
            const char *p = name + (name[0] == 'r' ? 5 : 4);
            int num = atoi(p);
            for(i = 0; i < DISKS_MAX; i++) {
                if(disks_targets[i] == num) return i;
            }
        }
        for(i = 0; i < DISKS_MAX; i++) {
            if(disks_targets[i] >= 1024 && disks_serials[disks_targets[i] - 1024][0]) {
                if(!strcmp(name, disks_serials[disks_targets[i] - 1024])) return i;
            }
        }
    }
#else
    {
        const char *name = device_id;
        if(!strncmp(device_id, "/dev/", 5)) name = device_id + 5;
        for(i = 0; i < DISKS_MAX; i++) {
            if(disks_targets[i] == -1) continue;
            if(!strncmp(disks_devs[i], name, sizeof(disks_devs[i]))) return i;
        }
    }
#endif

    return -1;
}

static const char *rl_stream_error(int code)
{
    switch(code) {
        case 2: return rl_lang_utf8(L_ENCZIPERR);
        case 3: return rl_lang_utf8(L_CMPZIPERR);
        case 4: return rl_lang_utf8(L_CMPERR);
        default: return rl_lang_utf8(L_SRCERR);
    }
}

static const char *rl_open_error(long code)
{
    if(code == -1) return rl_lang_utf8(L_TRGERR);
    if(code == -2) return rl_lang_utf8(L_UMOUNTERR);
    if(code == -4) return rl_lang_utf8(L_COMMERR);
    return rl_lang_utf8(L_OPENTRGERR);
}

#ifdef _WIN32
static DWORD WINAPI rl_writer_thread(LPVOID arg)
#else
static void *rl_writer_thread(void *arg)
#endif
{
    struct rl_job *job = (struct rl_job *)arg;
    stream_t ctx;
    int numberOfBytesRead;
    int needWrite;
    int targetId;

#ifdef _WIN32
    HANDLE dst;
    LARGE_INTEGER totalWritten;
    DWORD numberOfBytesWritten;
    DWORD numberOfBytesVerify;
#else
    int dst;
    int numberOfBytesWritten;
    int numberOfBytesVerify;
#endif

    memset(&ctx, 0, sizeof(ctx));
    rl_init_lang();

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

#ifdef _WIN32
    dst = (HANDLE)disks_open(targetId, ctx.fileSize);
    if(dst == NULL || dst == (HANDLE)-1 || dst == (HANDLE)-2 || dst == (HANDLE)-3 || dst == (HANDLE)-4) {
        rl_set_job_error(job, rl_open_error((long)dst));
        stream_close(&ctx);
        job->result = 1;
        job->done = 1;
        return NULL;
    }
    totalWritten.QuadPart = 0;
#else
    dst = (int)((long int)disks_open(targetId, ctx.fileSize));
    if(dst <= 0) {
        rl_set_job_error(job, rl_open_error((long)dst));
        stream_close(&ctx);
        job->result = 1;
        job->done = 1;
        return NULL;
    }
#endif

    rl_emit_progress(job, &ctx, 0);

    while(!job->cancel) {
        numberOfBytesRead = stream_read(&ctx);
        if(numberOfBytesRead < 0) {
            rl_set_job_error(job, rl_lang_utf8(L_RDSRCERR));
            job->result = 1;
            break;
        }
        if(numberOfBytesRead == 0) {
            if(!ctx.fileSize) ctx.fileSize = ctx.readSize;
            break;
        }
        errno = 0;
        needWrite = 1;

#ifdef _WIN32
        if(!force) {
            if(ReadFile(dst, ctx.verifyBuf, (DWORD)numberOfBytesRead, &numberOfBytesVerify, NULL) &&
                numberOfBytesRead == (int)numberOfBytesVerify &&
                !memcmp(ctx.buffer, ctx.verifyBuf, numberOfBytesRead)) {
                needWrite = 0;
                totalWritten.QuadPart += numberOfBytesVerify;
                rl_emit_progress(job, &ctx, 0);
            } else {
                SetFilePointerEx(dst, totalWritten, NULL, FILE_BEGIN);
            }
        }
        if(needWrite) {
            if(WriteFile(dst, ctx.buffer, (DWORD)numberOfBytesRead, &numberOfBytesWritten, NULL)) {
                if(job->verify) {
                    SetFilePointerEx(dst, totalWritten, NULL, FILE_BEGIN);
                    if(!ReadFile(dst, ctx.verifyBuf, numberOfBytesWritten, &numberOfBytesVerify, NULL) ||
                        numberOfBytesWritten != numberOfBytesVerify ||
                        memcmp(ctx.buffer, ctx.verifyBuf, numberOfBytesWritten)) {
                        rl_set_job_error(job, rl_lang_utf8(L_VRFYERR));
                        job->result = 1;
                        break;
                    }
                }
                totalWritten.QuadPart += numberOfBytesWritten;
                rl_emit_progress(job, &ctx, 0);
            } else {
                main_getErrorMessage();
                rl_set_job_error(job, rl_lang_utf8(L_WRTRGERR));
                job->result = 1;
                break;
            }
        }
#else
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
                        rl_set_job_error(job, rl_lang_utf8(L_VRFYERR));
                        job->result = 1;
                        break;
                    }
                }
                rl_emit_progress(job, &ctx, 0);
            } else {
                if(errno) main_getErrorMessage();
                rl_set_job_error(job, rl_lang_utf8(L_WRTRGERR));
                job->result = 1;
                break;
            }
        }
#endif
    }

    if(job->cancel && job->result == 0) {
        rl_set_job_error(job, "Cancelled");
        job->result = 2;
    }

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

    rl_init_lang();

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

#ifdef _WIN32
    job->thread = CreateThread(NULL, 0, rl_writer_thread, job, 0, NULL);
    if(!job->thread) {
        rl_set_last_error("Failed to start writer thread");
        free(job);
        return NULL;
    }
#else
    if(pthread_create(&job->thread, NULL, rl_writer_thread, job) != 0) {
        rl_set_last_error("Failed to start writer thread");
        free(job);
        return NULL;
    }
#endif

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
#ifdef _WIN32
    WaitForSingleObject(job->thread, INFINITE);
    CloseHandle(job->thread);
    job->thread = NULL;
    return job->result;
#else
    pthread_join(job->thread, NULL);
    return job->result;
#endif
}

void rl_free(rl_job *job)
{
    if(!job) return;
    if(!job->done) {
        job->cancel = 1;
#ifdef _WIN32
        WaitForSingleObject(job->thread, INFINITE);
        CloseHandle(job->thread);
        job->thread = NULL;
#else
        pthread_join(job->thread, NULL);
#endif
    }
    free(job);
}
