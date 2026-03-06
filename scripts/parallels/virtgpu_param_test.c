/*
 * virtgpu_param_test.c — Query all VirtGPU parameters and test resource creation
 *
 * Investigates why Mesa's RESOURCE_CREATE returns size=3145728/stride=4096
 * while standalone tests get size=0/stride=0 with identical parameters.
 *
 * Build:  gcc -O2 -o virtgpu_param_test virtgpu_param_test.c -ldrm
 * Run:    sudo ./virtgpu_param_test
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

/* =========================================================================
 * VirtGPU DRM ioctl definitions (from linux/virtgpu_drm.h)
 * ========================================================================= */

struct drm_virtgpu_getparam {
    uint64_t param;
    uint64_t value;
};

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
    uint32_t bo_handle;  /* output */
    uint32_t res_handle; /* output */
    uint32_t size;       /* output */
    uint32_t stride;     /* output */
};

struct drm_virtgpu_resource_create_blob {
    uint32_t blob_mem;
    uint32_t blob_flags;
    uint32_t bo_handle;  /* output */
    uint32_t res_handle; /* output */
    uint64_t size;
    uint32_t pad;
    uint32_t cmd_size;
    uint64_t cmd;
    uint64_t blob_id;
};

struct drm_virtgpu_resource_info {
    uint32_t bo_handle;
    uint32_t res_handle; /* output */
    uint32_t size;       /* output */
    uint32_t blob_mem;   /* output (kernel 5.16+) */
};

struct drm_virtgpu_map {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;  /* output */
};

struct drm_virtgpu_get_caps {
    uint32_t cap_set_id;
    uint32_t cap_set_ver;
    uint64_t addr;
    uint32_t size;
    uint32_t pad;
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

/* Ioctl numbers */
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

#define DRM_IOCTL_VIRTGPU_GETPARAM \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_GETPARAM, \
             struct drm_virtgpu_getparam)

#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE, \
             struct drm_virtgpu_resource_create)

#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE_BLOB, \
             struct drm_virtgpu_resource_create_blob)

#define DRM_IOCTL_VIRTGPU_RESOURCE_INFO \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_INFO, \
             struct drm_virtgpu_resource_info)

#define DRM_IOCTL_VIRTGPU_MAP \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_MAP, \
             struct drm_virtgpu_map)

#define DRM_IOCTL_VIRTGPU_GET_CAPS \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_GET_CAPS, \
             struct drm_virtgpu_get_caps)

#define DRM_IOCTL_VIRTGPU_CONTEXT_INIT \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_CONTEXT_INIT, \
             struct drm_virtgpu_context_init)

/* Parameter IDs */
#define VIRTGPU_PARAM_3D_FEATURES      1
#define VIRTGPU_PARAM_CAPSET_QUERY_FIX 2
#define VIRTGPU_PARAM_RESOURCE_BLOB    3
#define VIRTGPU_PARAM_HOST_VISIBLE     4
#define VIRTGPU_PARAM_CROSS_DEVICE     5
#define VIRTGPU_PARAM_CONTEXT_INIT     6
#define VIRTGPU_PARAM_SUPPORTED_CAPSET_IDS 7

/* Blob constants */
#define VIRTGPU_BLOB_MEM_GUEST           1
#define VIRTGPU_BLOB_MEM_HOST3D          2
#define VIRTGPU_BLOB_MEM_HOST3D_GUEST    3

#define VIRTGPU_BLOB_FLAG_USE_MAPPABLE   (1 << 0)
#define VIRTGPU_BLOB_FLAG_USE_SHAREABLE  (1 << 1)
#define VIRTGPU_BLOB_FLAG_USE_CROSS_DEVICE (1 << 2)

/* Pipe/format constants */
#define PIPE_TEXTURE_2D    2
#define PIPE_FORMAT_B8G8R8X8_UNORM    2
#define PIPE_BIND_RENDER_TARGET   0x002
#define PIPE_BIND_SAMPLER_VIEW    0x008
#define PIPE_BIND_SCANOUT         0x40000
#define PIPE_BIND_SHARED          0x100000

/* =========================================================================
 * Main
 * ========================================================================= */

int main(void)
{
    printf("=== VirtGPU Parameter & Resource Creation Test ===\n\n");

    /* Open DRM device */
    const char *cards[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};
    int fd = -1;
    for (int i = 0; cards[i]; i++) {
        fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (fd >= 0) {
            printf("Opened %s (fd=%d)\n", cards[i], fd);
            break;
        }
    }
    if (fd < 0) {
        fprintf(stderr, "Cannot open any DRM device\n");
        return 1;
    }

    /* ===================================================================
     * Part 1: Query ALL known parameters
     * =================================================================== */
    printf("\n=== GETPARAM queries ===\n");

    struct {
        uint64_t id;
        const char *name;
    } params[] = {
        {1, "3D_FEATURES"},
        {2, "CAPSET_QUERY_FIX"},
        {3, "RESOURCE_BLOB"},
        {4, "HOST_VISIBLE"},
        {5, "CROSS_DEVICE"},
        {6, "CONTEXT_INIT"},
        {7, "SUPPORTED_CAPSET_IDS"},
        {8, "PARAM_8 (unknown)"},
        {9, "PARAM_9 (unknown)"},
        {10, "PARAM_10 (unknown)"},
    };

    uint64_t param_values[11] = {0};
    int has_3d = 0, has_blob = 0, has_host_visible = 0;

    for (int i = 0; i < 10; i++) {
        uint64_t value = 0;
        struct drm_virtgpu_getparam gp;
        memset(&gp, 0, sizeof(gp));
        gp.param = params[i].id;
        gp.value = (uint64_t)(uintptr_t)&value;

        int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_GETPARAM, &gp);
        if (ret < 0) {
            printf("  PARAM %2lu %-25s = UNSUPPORTED (errno=%d: %s)\n",
                   (unsigned long)params[i].id, params[i].name,
                   errno, strerror(errno));
        } else {
            printf("  PARAM %2lu %-25s = %lu (0x%lx)\n",
                   (unsigned long)params[i].id, params[i].name,
                   (unsigned long)value, (unsigned long)value);
            param_values[params[i].id] = value;
        }

        if (params[i].id == 1 && ret == 0) has_3d = (value != 0);
        if (params[i].id == 3 && ret == 0) has_blob = (value != 0);
        if (params[i].id == 4 && ret == 0) has_host_visible = (value != 0);
    }

    printf("\nSummary: 3D=%d, BLOB=%d, HOST_VISIBLE=%d\n", has_3d, has_blob, has_host_visible);

    /* ===================================================================
     * Part 2: GET_CAPS - query capability sets
     * =================================================================== */
    printf("\n=== GET_CAPS ===\n");
    {
        /* Try capset 1 (VIRGL) and capset 2 (VIRGL2) */
        for (uint32_t capset_id = 1; capset_id <= 2; capset_id++) {
            uint8_t caps[1024];
            memset(caps, 0, sizeof(caps));

            struct drm_virtgpu_get_caps gc;
            memset(&gc, 0, sizeof(gc));
            gc.cap_set_id = capset_id;
            gc.cap_set_ver = 0;
            gc.addr = (uint64_t)(uintptr_t)caps;
            gc.size = sizeof(caps);

            int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_GET_CAPS, &gc);
            if (ret < 0) {
                printf("  Capset %u: UNSUPPORTED (errno=%d: %s)\n",
                       capset_id, errno, strerror(errno));
            } else {
                printf("  Capset %u: OK (size=%u)\n", capset_id, gc.size);
                /* Dump first 64 bytes */
                printf("  First 64 bytes: ");
                for (int j = 0; j < 64 && j < (int)gc.size; j++)
                    printf("%02x", caps[j]);
                printf("\n");
            }
        }
    }

    /* ===================================================================
     * Part 3: Get display mode for sizing
     * =================================================================== */
    uint32_t width = 1024, height = 768;
    {
        drmModeResPtr res = drmModeGetResources(fd);
        if (res) {
            for (int c = 0; c < res->count_connectors; c++) {
                drmModeConnectorPtr conn = drmModeGetConnector(fd, res->connectors[c]);
                if (conn && conn->connection == DRM_MODE_CONNECTED && conn->count_modes > 0) {
                    width = conn->modes[0].hdisplay;
                    height = conn->modes[0].vdisplay;
                    printf("\nDisplay mode: %ux%u\n", width, height);
                    drmModeFreeConnector(conn);
                    break;
                }
                if (conn) drmModeFreeConnector(conn);
            }
            drmModeFreeResources(res);
        }
    }

    /* ===================================================================
     * Part 4: RESOURCE_CREATE tests (standard 3D path)
     * =================================================================== */
    printf("\n=== RESOURCE_CREATE Tests ===\n");

    /* Test A: Exact Mesa parameters (what gl_display does) */
    {
        printf("\n--- Test A: 3D resource, flags=0 (standard Mesa path) ---\n");
        struct drm_virtgpu_resource_create rc;
        memset(&rc, 0, sizeof(rc));
        rc.target = PIPE_TEXTURE_2D;
        rc.format = PIPE_FORMAT_B8G8R8X8_UNORM;
        rc.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW |
                  PIPE_BIND_SCANOUT | PIPE_BIND_SHARED;
        rc.width = width;
        rc.height = height;
        rc.depth = 1;
        rc.array_size = 1;
        rc.last_level = 0;
        rc.nr_samples = 0;
        rc.flags = 0;
        /* Key: pre-fill size to see if kernel uses it */
        rc.size = 0;
        rc.stride = 0;
        rc.bo_handle = 0;
        rc.res_handle = 0;

        printf("  INPUT: target=%u format=%u bind=0x%x %ux%ux%u flags=0x%x size=%u stride=%u\n",
               rc.target, rc.format, rc.bind, rc.width, rc.height, rc.depth,
               rc.flags, rc.size, rc.stride);

        int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc);
        if (ret < 0) {
            printf("  FAILED: %s\n", strerror(errno));
        } else {
            printf("  OUTPUT: bo_handle=%u res_handle=%u size=%u stride=%u\n",
                   rc.bo_handle, rc.res_handle, rc.size, rc.stride);
            printf("  Expected: size=%u (width*4*height) stride=%u (width*4)\n",
                   width * 4 * height, width * 4);

            /* Try RESOURCE_INFO on this handle */
            struct drm_virtgpu_resource_info ri;
            memset(&ri, 0, sizeof(ri));
            ri.bo_handle = rc.bo_handle;
            ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri);
            if (ret < 0) {
                printf("  RESOURCE_INFO: FAILED — %s\n", strerror(errno));
            } else {
                printf("  RESOURCE_INFO: res_handle=%u size=%u blob_mem=%u\n",
                       ri.res_handle, ri.size, ri.blob_mem);
            }

            /* Try MAP */
            struct drm_virtgpu_map vmap;
            memset(&vmap, 0, sizeof(vmap));
            vmap.handle = rc.bo_handle;
            ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_MAP, &vmap);
            if (ret < 0) {
                printf("  MAP: FAILED — %s\n", strerror(errno));
            } else {
                printf("  MAP: offset=0x%lx\n", (unsigned long)vmap.offset);
                /* Try mmap with expected size */
                uint32_t map_size = width * 4 * height;
                void *ptr = mmap(NULL, map_size, PROT_READ | PROT_WRITE,
                                 MAP_SHARED, fd, vmap.offset);
                if (ptr == MAP_FAILED) {
                    printf("  mmap(%u bytes): FAILED — %s\n", map_size, strerror(errno));
                } else {
                    printf("  mmap(%u bytes): OK at %p\n", map_size, ptr);
                    /* Check if memory is accessible */
                    uint32_t *pixels = (uint32_t *)ptr;
                    printf("  pixel[0] = 0x%08x\n", pixels[0]);
                    munmap(ptr, map_size);
                }
            }
        }
    }

    /* Test B: With size pre-filled (like dumb buffer path) */
    {
        printf("\n--- Test B: 3D resource, size pre-filled = width*4*height ---\n");
        struct drm_virtgpu_resource_create rc;
        memset(&rc, 0, sizeof(rc));
        rc.target = PIPE_TEXTURE_2D;
        rc.format = PIPE_FORMAT_B8G8R8X8_UNORM;
        rc.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW |
                  PIPE_BIND_SCANOUT | PIPE_BIND_SHARED;
        rc.width = width;
        rc.height = height;
        rc.depth = 1;
        rc.array_size = 1;
        rc.last_level = 0;
        rc.nr_samples = 0;
        rc.flags = 0;
        /* Pre-fill size */
        rc.size = width * 4 * height;
        rc.stride = 0;

        printf("  INPUT: size=%u (pre-filled)\n", rc.size);

        int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc);
        if (ret < 0) {
            printf("  FAILED: %s\n", strerror(errno));
        } else {
            printf("  OUTPUT: bo_handle=%u res_handle=%u size=%u stride=%u\n",
                   rc.bo_handle, rc.res_handle, rc.size, rc.stride);

            struct drm_virtgpu_resource_info ri;
            memset(&ri, 0, sizeof(ri));
            ri.bo_handle = rc.bo_handle;
            ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri);
            if (ret < 0)
                printf("  RESOURCE_INFO: FAILED — %s\n", strerror(errno));
            else
                printf("  RESOURCE_INFO: res_handle=%u size=%u blob_mem=%u\n",
                       ri.res_handle, ri.size, ri.blob_mem);
        }
    }

    /* Test C: Minimal bind flags (just RENDER_TARGET) */
    {
        printf("\n--- Test C: 3D resource, bind=RENDER_TARGET only ---\n");
        struct drm_virtgpu_resource_create rc;
        memset(&rc, 0, sizeof(rc));
        rc.target = PIPE_TEXTURE_2D;
        rc.format = PIPE_FORMAT_B8G8R8X8_UNORM;
        rc.bind = PIPE_BIND_RENDER_TARGET;
        rc.width = width;
        rc.height = height;
        rc.depth = 1;
        rc.array_size = 1;
        rc.flags = 0;
        rc.size = 0;

        int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc);
        if (ret < 0) {
            printf("  FAILED: %s\n", strerror(errno));
        } else {
            printf("  OUTPUT: bo_handle=%u res_handle=%u size=%u stride=%u\n",
                   rc.bo_handle, rc.res_handle, rc.size, rc.stride);
        }
    }

    /* ===================================================================
     * Part 5: RESOURCE_CREATE_BLOB tests (if supported)
     * =================================================================== */
    if (has_blob) {
        printf("\n=== RESOURCE_CREATE_BLOB Tests ===\n");

        /* Test D: Guest blob (VIRTGPU_BLOB_MEM_GUEST) */
        {
            printf("\n--- Test D: BLOB_MEM_GUEST, mappable+shareable ---\n");
            struct drm_virtgpu_resource_create_blob rcb;
            memset(&rcb, 0, sizeof(rcb));
            rcb.blob_mem = VIRTGPU_BLOB_MEM_GUEST;
            rcb.blob_flags = VIRTGPU_BLOB_FLAG_USE_MAPPABLE | VIRTGPU_BLOB_FLAG_USE_SHAREABLE;
            rcb.size = width * 4 * height;
            rcb.cmd_size = 0;
            rcb.cmd = 0;
            rcb.blob_id = 0;

            printf("  INPUT: blob_mem=%u blob_flags=0x%x size=%lu\n",
                   rcb.blob_mem, rcb.blob_flags, (unsigned long)rcb.size);

            int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB, &rcb);
            if (ret < 0) {
                printf("  FAILED: %s\n", strerror(errno));
            } else {
                printf("  OUTPUT: bo_handle=%u res_handle=%u\n",
                       rcb.bo_handle, rcb.res_handle);

                /* Try MAP */
                struct drm_virtgpu_map vmap;
                memset(&vmap, 0, sizeof(vmap));
                vmap.handle = rcb.bo_handle;
                ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_MAP, &vmap);
                if (ret < 0) {
                    printf("  MAP: FAILED — %s\n", strerror(errno));
                } else {
                    printf("  MAP: offset=0x%lx\n", (unsigned long)vmap.offset);
                    void *ptr = mmap(NULL, rcb.size, PROT_READ | PROT_WRITE,
                                     MAP_SHARED, fd, vmap.offset);
                    if (ptr == MAP_FAILED)
                        printf("  mmap: FAILED — %s\n", strerror(errno));
                    else {
                        printf("  mmap(%lu bytes): OK\n", (unsigned long)rcb.size);
                        munmap(ptr, rcb.size);
                    }
                }

                /* RESOURCE_INFO */
                struct drm_virtgpu_resource_info ri;
                memset(&ri, 0, sizeof(ri));
                ri.bo_handle = rcb.bo_handle;
                ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri);
                if (ret < 0)
                    printf("  RESOURCE_INFO: FAILED — %s\n", strerror(errno));
                else
                    printf("  RESOURCE_INFO: res_handle=%u size=%u blob_mem=%u\n",
                           ri.res_handle, ri.size, ri.blob_mem);
            }
        }

        /* Test E: Host3D blob */
        {
            printf("\n--- Test E: BLOB_MEM_HOST3D, mappable ---\n");
            struct drm_virtgpu_resource_create_blob rcb;
            memset(&rcb, 0, sizeof(rcb));
            rcb.blob_mem = VIRTGPU_BLOB_MEM_HOST3D;
            rcb.blob_flags = VIRTGPU_BLOB_FLAG_USE_MAPPABLE;
            rcb.size = width * 4 * height;
            rcb.cmd_size = 0;
            rcb.cmd = 0;
            rcb.blob_id = 1;

            int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB, &rcb);
            if (ret < 0) {
                printf("  FAILED: %s\n", strerror(errno));
            } else {
                printf("  OUTPUT: bo_handle=%u res_handle=%u\n",
                       rcb.bo_handle, rcb.res_handle);

                struct drm_virtgpu_resource_info ri;
                memset(&ri, 0, sizeof(ri));
                ri.bo_handle = rcb.bo_handle;
                ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri);
                if (ret < 0)
                    printf("  RESOURCE_INFO: FAILED — %s\n", strerror(errno));
                else
                    printf("  RESOURCE_INFO: res_handle=%u size=%u blob_mem=%u\n",
                           ri.res_handle, ri.size, ri.blob_mem);
            }
        }

        /* Test F: Host3D+Guest blob */
        {
            printf("\n--- Test F: BLOB_MEM_HOST3D_GUEST, mappable+shareable ---\n");
            struct drm_virtgpu_resource_create_blob rcb;
            memset(&rcb, 0, sizeof(rcb));
            rcb.blob_mem = VIRTGPU_BLOB_MEM_HOST3D_GUEST;
            rcb.blob_flags = VIRTGPU_BLOB_FLAG_USE_MAPPABLE | VIRTGPU_BLOB_FLAG_USE_SHAREABLE;
            rcb.size = width * 4 * height;
            rcb.cmd_size = 0;
            rcb.cmd = 0;
            rcb.blob_id = 2;

            int ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB, &rcb);
            if (ret < 0) {
                printf("  FAILED: %s\n", strerror(errno));
            } else {
                printf("  OUTPUT: bo_handle=%u res_handle=%u\n",
                       rcb.bo_handle, rcb.res_handle);

                struct drm_virtgpu_resource_info ri;
                memset(&ri, 0, sizeof(ri));
                ri.bo_handle = rcb.bo_handle;
                ret = drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri);
                if (ret < 0)
                    printf("  RESOURCE_INFO: FAILED — %s\n", strerror(errno));
                else
                    printf("  RESOURCE_INFO: res_handle=%u size=%u blob_mem=%u\n",
                           ri.res_handle, ri.size, ri.blob_mem);
            }
        }
    } else {
        printf("\n=== RESOURCE_CREATE_BLOB: SKIPPED (not supported) ===\n");
    }

    /* ===================================================================
     * Part 6: Check ioctl numbers for reference
     * =================================================================== */
    printf("\n=== Ioctl Numbers ===\n");
    printf("  DRM_IOCTL_VIRTGPU_RESOURCE_CREATE      = 0x%lx\n",
           (unsigned long)DRM_IOCTL_VIRTGPU_RESOURCE_CREATE);
    printf("  DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB = 0x%lx\n",
           (unsigned long)DRM_IOCTL_VIRTGPU_RESOURCE_CREATE_BLOB);
    printf("  DRM_IOCTL_VIRTGPU_RESOURCE_INFO        = 0x%lx\n",
           (unsigned long)DRM_IOCTL_VIRTGPU_RESOURCE_INFO);

    printf("\nDone.\n");
    close(fd);
    return 0;
}
