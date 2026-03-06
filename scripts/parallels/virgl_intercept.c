/*
 * virgl_intercept.c — LD_PRELOAD interceptor for VirtGPU DRM ioctls
 *
 * Captures and hex-dumps all VirtGPU ioctl calls from Mesa/EGL programs.
 * Intercepts: RESOURCE_CREATE, RESOURCE_CREATE_BLOB, EXECBUFFER,
 *             GETPARAM, GET_CAPS, RESOURCE_INFO, MAP, CONTEXT_INIT
 *
 * Build:
 *   gcc -shared -fPIC -o virgl_intercept.so virgl_intercept.c -ldl
 *
 * Usage:
 *   LD_PRELOAD=./virgl_intercept.so ./gl_display 2>&1 | tee intercept.log
 */

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <dlfcn.h>
#include <sys/ioctl.h>
#include <errno.h>

/* DRM base */
#define DRM_COMMAND_BASE 0x40
#define DRM_IOCTL_BASE   'd'

/* VirtGPU ioctl command offsets */
#define DRM_VIRTGPU_MAP              0x01
#define DRM_VIRTGPU_EXECBUFFER       0x02
#define DRM_VIRTGPU_GETPARAM         0x03
#define DRM_VIRTGPU_RESOURCE_CREATE  0x04
#define DRM_VIRTGPU_RESOURCE_INFO    0x05
#define DRM_VIRTGPU_TRANSFER_FROM_HOST 0x06
#define DRM_VIRTGPU_TRANSFER_TO_HOST 0x07
#define DRM_VIRTGPU_WAIT             0x08
#define DRM_VIRTGPU_GET_CAPS         0x09
#define DRM_VIRTGPU_RESOURCE_CREATE_BLOB 0x0a
#define DRM_VIRTGPU_CONTEXT_INIT     0x0b

/* Struct definitions */
struct drm_virtgpu_resource_create {
    uint32_t target;
    uint32_t format;
    uint32_t bind;
    uint32_t width;
    uint32_t height;
    uint32_t depth;
    uint32_t array_size;
    uint32_t last_level;
    uint32_t nr_samples;
    uint32_t flags;
    uint32_t bo_handle;
    uint32_t res_handle;
    uint32_t size;
    uint32_t stride;
};

struct drm_virtgpu_resource_create_blob {
    uint32_t blob_mem;
    uint32_t blob_flags;
    uint32_t bo_handle;
    uint32_t res_handle;
    uint64_t size;
    uint32_t pad;
    uint32_t cmd_size;
    uint64_t cmd;
    uint64_t blob_id;
};

struct drm_virtgpu_execbuffer {
    uint32_t flags;
    uint32_t size;
    uint64_t command;
    uint64_t bo_handles;
    uint32_t num_bo_handles;
    int32_t  fence_fd;
};

struct drm_virtgpu_getparam {
    uint64_t param;
    uint64_t value;
};

struct drm_virtgpu_get_caps {
    uint32_t cap_set_id;
    uint32_t cap_set_ver;
    uint64_t addr;
    uint32_t size;
    uint32_t pad;
};

struct drm_virtgpu_resource_info {
    uint32_t bo_handle;
    uint32_t res_handle;
    uint32_t size;
    uint32_t blob_mem;
};

struct drm_virtgpu_map {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;
};

struct drm_virtgpu_context_init {
    uint32_t num_params;
    uint32_t pad;
    uint64_t ctx_set_params;
};

struct drm_virtgpu_context_set_param {
    uint64_t param;
    uint64_t value;
};

/* Helper to extract DRM command number from ioctl request */
static inline uint32_t drm_cmd_nr(unsigned long request) {
    return ((request >> 0) & 0xFF) - DRM_COMMAND_BASE;
}

/* Check if this is a DRM ioctl */
static inline int is_drm_ioctl(unsigned long request) {
    return ((request >> 8) & 0xFF) == DRM_IOCTL_BASE;
}

static int intercept_count = 0;

typedef int (*real_ioctl_t)(int fd, unsigned long request, ...);
static real_ioctl_t real_ioctl = NULL;

static void init_real_ioctl(void) {
    if (!real_ioctl) {
        real_ioctl = (real_ioctl_t)dlsym(RTLD_NEXT, "ioctl");
    }
}

int ioctl(int fd, unsigned long request, ...) {
    init_real_ioctl();

    /* Extract variadic arg */
    void *arg;
    __builtin_va_list ap;
    __builtin_va_start(ap, request);
    arg = __builtin_va_arg(ap, void *);
    __builtin_va_end(ap);

    if (!is_drm_ioctl(request)) {
        return real_ioctl(fd, request, arg);
    }

    uint32_t cmd = drm_cmd_nr(request);
    int seq = ++intercept_count;

    switch (cmd) {
    case DRM_VIRTGPU_GETPARAM: {
        struct drm_virtgpu_getparam *gp = arg;
        int ret = real_ioctl(fd, request, arg);
        uint64_t val = 0;
        if (ret == 0 && gp->value)
            val = *(uint64_t *)(uintptr_t)gp->value;
        fprintf(stderr, "[intercept#%d] GETPARAM: param=%lu value=%lu ret=%d\n",
                seq, (unsigned long)gp->param, (unsigned long)val, ret);
        return ret;
    }

    case DRM_VIRTGPU_GET_CAPS: {
        struct drm_virtgpu_get_caps *gc = arg;
        fprintf(stderr, "[intercept#%d] GET_CAPS: capset_id=%u ver=%u size=%u\n",
                seq, gc->cap_set_id, gc->cap_set_ver, gc->size);
        int ret = real_ioctl(fd, request, arg);
        fprintf(stderr, "[intercept#%d] GET_CAPS: ret=%d\n", seq, ret);
        if (ret == 0 && gc->addr && gc->size > 0) {
            uint8_t *caps = (uint8_t *)(uintptr_t)gc->addr;
            fprintf(stderr, "[intercept#%d] GET_CAPS first 64 bytes: ", seq);
            for (uint32_t i = 0; i < 64 && i < gc->size; i++)
                fprintf(stderr, "%02x", caps[i]);
            fprintf(stderr, "\n");
        }
        return ret;
    }

    case DRM_VIRTGPU_RESOURCE_CREATE: {
        struct drm_virtgpu_resource_create *rc = arg;
        fprintf(stderr, "[intercept#%d] RESOURCE_CREATE INPUT:\n", seq);
        fprintf(stderr, "  target=%u format=%u bind=0x%x\n",
                rc->target, rc->format, rc->bind);
        fprintf(stderr, "  %ux%ux%u array=%u levels=%u samples=%u\n",
                rc->width, rc->height, rc->depth,
                rc->array_size, rc->last_level, rc->nr_samples);
        fprintf(stderr, "  flags=0x%x size=%u stride=%u\n",
                rc->flags, rc->size, rc->stride);

        int ret = real_ioctl(fd, request, arg);

        fprintf(stderr, "[intercept#%d] RESOURCE_CREATE OUTPUT: ret=%d\n", seq, ret);
        if (ret == 0) {
            fprintf(stderr, "  bo_handle=%u res_handle=%u size=%u stride=%u\n",
                    rc->bo_handle, rc->res_handle, rc->size, rc->stride);
        } else {
            fprintf(stderr, "  errno=%d (%s)\n", errno, strerror(errno));
        }
        return ret;
    }

    case DRM_VIRTGPU_RESOURCE_CREATE_BLOB: {
        struct drm_virtgpu_resource_create_blob *rcb = arg;
        fprintf(stderr, "[intercept#%d] RESOURCE_CREATE_BLOB INPUT:\n", seq);
        fprintf(stderr, "  blob_mem=%u blob_flags=0x%x size=%lu\n",
                rcb->blob_mem, rcb->blob_flags, (unsigned long)rcb->size);
        fprintf(stderr, "  cmd_size=%u blob_id=%lu\n",
                rcb->cmd_size, (unsigned long)rcb->blob_id);
        if (rcb->cmd_size > 0 && rcb->cmd) {
            uint32_t *cmds = (uint32_t *)(uintptr_t)rcb->cmd;
            uint32_t ndwords = rcb->cmd_size / 4;
            fprintf(stderr, "  cmd payload (%u dwords):", ndwords);
            for (uint32_t i = 0; i < ndwords && i < 32; i++)
                fprintf(stderr, " 0x%08x", cmds[i]);
            fprintf(stderr, "\n");
        }

        int ret = real_ioctl(fd, request, arg);

        fprintf(stderr, "[intercept#%d] RESOURCE_CREATE_BLOB OUTPUT: ret=%d\n", seq, ret);
        if (ret == 0) {
            fprintf(stderr, "  bo_handle=%u res_handle=%u\n",
                    rcb->bo_handle, rcb->res_handle);
        } else {
            fprintf(stderr, "  errno=%d (%s)\n", errno, strerror(errno));
        }
        return ret;
    }

    case DRM_VIRTGPU_EXECBUFFER: {
        struct drm_virtgpu_execbuffer *eb = arg;
        uint32_t ndwords = eb->size / 4;
        fprintf(stderr, "[intercept#%d] EXECBUFFER: size=%u (%u dwords) num_bos=%u flags=0x%x\n",
                seq, eb->size, ndwords, eb->num_bo_handles, eb->flags);

        /* Dump command payload */
        if (eb->command && ndwords > 0) {
            uint32_t *cmds = (uint32_t *)(uintptr_t)eb->command;
            fprintf(stderr, "[intercept#%d] EXECBUFFER payload:\n", seq);
            for (uint32_t i = 0; i < ndwords; i++) {
                fprintf(stderr, "  +%04u: 0x%08x\n", i * 4, cmds[i]);
            }
        }

        /* Dump BO handles */
        if (eb->bo_handles && eb->num_bo_handles > 0) {
            uint32_t *bos = (uint32_t *)(uintptr_t)eb->bo_handles;
            fprintf(stderr, "[intercept#%d] EXECBUFFER bo_handles:", seq);
            for (uint32_t i = 0; i < eb->num_bo_handles; i++)
                fprintf(stderr, " %u", bos[i]);
            fprintf(stderr, "\n");
        }

        int ret = real_ioctl(fd, request, arg);
        fprintf(stderr, "[intercept#%d] EXECBUFFER: ret=%d fence_fd=%d\n",
                seq, ret, eb->fence_fd);
        return ret;
    }

    case DRM_VIRTGPU_RESOURCE_INFO: {
        struct drm_virtgpu_resource_info *ri = arg;
        uint32_t in_handle = ri->bo_handle;
        int ret = real_ioctl(fd, request, arg);
        fprintf(stderr, "[intercept#%d] RESOURCE_INFO: bo=%u -> res=%u size=%u blob_mem=%u ret=%d\n",
                seq, in_handle, ri->res_handle, ri->size, ri->blob_mem, ret);
        return ret;
    }

    case DRM_VIRTGPU_MAP: {
        struct drm_virtgpu_map *vm = arg;
        uint32_t in_handle = vm->handle;
        int ret = real_ioctl(fd, request, arg);
        fprintf(stderr, "[intercept#%d] MAP: handle=%u -> offset=0x%lx ret=%d\n",
                seq, in_handle, (unsigned long)vm->offset, ret);
        if (ret < 0)
            fprintf(stderr, "[intercept#%d] MAP errno=%d (%s)\n", seq, errno, strerror(errno));
        return ret;
    }

    case DRM_VIRTGPU_CONTEXT_INIT: {
        struct drm_virtgpu_context_init *ci = arg;
        fprintf(stderr, "[intercept#%d] CONTEXT_INIT: num_params=%u\n",
                seq, ci->num_params);
        if (ci->ctx_set_params && ci->num_params > 0) {
            struct drm_virtgpu_context_set_param *params =
                (struct drm_virtgpu_context_set_param *)(uintptr_t)ci->ctx_set_params;
            for (uint32_t i = 0; i < ci->num_params; i++) {
                fprintf(stderr, "  param[%u]: key=%lu value=%lu\n",
                        i, (unsigned long)params[i].param,
                        (unsigned long)params[i].value);
            }
        }
        int ret = real_ioctl(fd, request, arg);
        fprintf(stderr, "[intercept#%d] CONTEXT_INIT: ret=%d\n", seq, ret);
        return ret;
    }

    case DRM_VIRTGPU_TRANSFER_FROM_HOST:
        fprintf(stderr, "[intercept#%d] TRANSFER_FROM_HOST\n", seq);
        break;

    case DRM_VIRTGPU_TRANSFER_TO_HOST:
        fprintf(stderr, "[intercept#%d] TRANSFER_TO_HOST\n", seq);
        break;

    case DRM_VIRTGPU_WAIT:
        fprintf(stderr, "[intercept#%d] WAIT\n", seq);
        break;

    default:
        fprintf(stderr, "[intercept#%d] DRM cmd=0x%x\n", seq, cmd);
        break;
    }

    return real_ioctl(fd, request, arg);
}
