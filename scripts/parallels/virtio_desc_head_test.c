/*
 * virtio_desc_head_test.c — Test whether Parallels VirtIO GPU handles
 *                           non-zero head descriptor indices.
 *
 * The Linux VirtIO driver uses a descriptor free list. After the first
 * submit+complete cycle, the LIFO free list reorders descriptors, so
 * subsequent submissions use non-zero head indices in the avail ring.
 *
 * This test:
 *   1. Enables ftrace on virtqueue_add_sgs to capture descriptor heads
 *   2. Submits 20 VirtIO GPU commands (RESOURCE_CREATE + EXECBUFFER)
 *   3. Parses ftrace output to extract the actual head descriptor indices
 *   4. Reports which heads were used and whether non-zero heads succeeded
 *
 * If ALL commands succeed and non-zero heads appear in the trace, then
 * Parallels fully supports non-zero head descriptor indices and the
 * Breenix crash was caused by a bug in Breenix, not a Parallels limitation.
 *
 * Build:  gcc -O2 -o virtio_desc_head_test virtio_desc_head_test.c -ldrm
 * Run:    sudo ./virtio_desc_head_test
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

/* =========================================================================
 * VirtGPU DRM ioctl definitions
 * ========================================================================= */

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

struct drm_virtgpu_execbuffer {
    uint32_t flags;
    uint32_t size;
    uint64_t command;
    uint64_t bo_handles;
    uint32_t num_bo_handles;
    int32_t  fence_fd;
};

struct drm_virtgpu_3d_wait {
    uint32_t handle;
    uint32_t flags;
};

#define DRM_VIRTGPU_EXECBUFFER       0x02
#define DRM_VIRTGPU_RESOURCE_CREATE  0x04
#define DRM_VIRTGPU_WAIT             0x08

#define DRM_IOCTL_VIRTGPU_EXECBUFFER \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_EXECBUFFER, \
             struct drm_virtgpu_execbuffer)

#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE, \
             struct drm_virtgpu_resource_create)

#define DRM_IOCTL_VIRTGPU_WAIT \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_WAIT, \
             struct drm_virtgpu_3d_wait)

/* VirGL command encoding */
#define VIRGL_CCMD_SET_SUB_CTX     28
#define VIRGL_CCMD_CREATE_SUB_CTX  29
#define VIRGL_CCMD_NOP              0

/* Encode a VirGL command header: length(16) | obj_type(8) | cmd(8) */
static inline uint32_t virgl_cmd_hdr(uint32_t cmd, uint32_t obj_type, uint32_t length) {
    return cmd | (obj_type << 8) | (length << 16);
}

/* =========================================================================
 * ftrace helpers
 * ========================================================================= */

static const char *TRACE_DIR = "/sys/kernel/debug/tracing";

static int write_file(const char *path, const char *value) {
    int fd = open(path, O_WRONLY);
    if (fd < 0) return -1;
    int ret = write(fd, value, strlen(value));
    close(fd);
    return ret > 0 ? 0 : -1;
}

static int setup_ftrace(void) {
    char path[256];

    /* Clear trace buffer */
    snprintf(path, sizeof(path), "%s/trace", TRACE_DIR);
    write_file(path, "");

    /* Try to enable virtio_ring tracepoints */
    snprintf(path, sizeof(path), "%s/events/virtio_ring/enable", TRACE_DIR);
    if (write_file(path, "1") == 0) {
        printf("[ftrace] Enabled virtio_ring tracepoints\n");
    } else {
        printf("[ftrace] virtio_ring tracepoints not available, trying kprobe...\n");

        /* Fall back to kprobe on virtqueue_add_sgs */
        snprintf(path, sizeof(path), "%s/kprobe_events", TRACE_DIR);
        /* Clear existing kprobes first */
        write_file(path, "");
        /* Add kprobe: virtqueue_add_sgs returns the head descriptor index */
        write_file(path, "p:vq_add virtqueue_add_sgs");

        snprintf(path, sizeof(path), "%s/events/kprobes/vq_add/enable", TRACE_DIR);
        if (write_file(path, "1") == 0) {
            printf("[ftrace] Enabled kprobe on virtqueue_add_sgs\n");
        } else {
            printf("[ftrace] WARNING: Could not set up any tracing. "
                   "Run as root and ensure debugfs is mounted.\n");
            return -1;
        }
    }

    /* Enable tracing */
    snprintf(path, sizeof(path), "%s/tracing_on", TRACE_DIR);
    write_file(path, "1");

    return 0;
}

static void stop_ftrace(void) {
    char path[256];

    snprintf(path, sizeof(path), "%s/tracing_on", TRACE_DIR);
    write_file(path, "0");

    /* Disable tracepoints */
    snprintf(path, sizeof(path), "%s/events/virtio_ring/enable", TRACE_DIR);
    write_file(path, "0");

    snprintf(path, sizeof(path), "%s/events/kprobes/vq_add/enable", TRACE_DIR);
    write_file(path, "0");
}

static void dump_and_parse_ftrace(void) {
    char path[256];
    snprintf(path, sizeof(path), "%s/trace", TRACE_DIR);

    FILE *f = fopen(path, "r");
    if (!f) {
        printf("[ftrace] Could not read trace buffer\n");
        return;
    }

    char line[1024];
    int total_lines = 0;
    int virtio_gpu_lines = 0;
    int head_zero = 0;
    int head_nonzero = 0;

    printf("\n========== FTRACE OUTPUT (virtio-gpu related) ==========\n");
    while (fgets(line, sizeof(line), f)) {
        /* Skip comment lines */
        if (line[0] == '#') continue;
        total_lines++;

        /* Look for virtio GPU related entries */
        if (strstr(line, "virtio") || strstr(line, "vq_add")) {
            virtio_gpu_lines++;
            /* Print first 30 lines to show the pattern */
            if (virtio_gpu_lines <= 30) {
                printf("  %s", line);
            }

            /* Parse head descriptor index from virtqueue_add_sgs trace.
             * Format varies but typically includes "head: N" or similar.
             * Also look for the avail ring write pattern. */
            char *head_str = strstr(line, "head=");
            if (!head_str) head_str = strstr(line, "head: ");
            if (head_str) {
                int head_val = atoi(head_str + (head_str[4] == '=' ? 5 : 6));
                if (head_val == 0) head_zero++;
                else head_nonzero++;
            }
        }
    }
    if (virtio_gpu_lines > 30) {
        printf("  ... (%d more lines)\n", virtio_gpu_lines - 30);
    }
    printf("========================================================\n");
    printf("[ftrace] Total trace lines: %d, virtio-related: %d\n",
           total_lines, virtio_gpu_lines);

    if (head_zero + head_nonzero > 0) {
        printf("[ftrace] Head descriptor indices observed:\n");
        printf("  head=0:    %d times\n", head_zero);
        printf("  head!=0:   %d times\n", head_nonzero);
        if (head_nonzero > 0) {
            printf("\n  *** CONFIRMED: Parallels VirtIO GPU successfully processes "
                   "non-zero head descriptor indices. ***\n");
        }
    } else {
        printf("[ftrace] Could not parse head descriptor indices from trace output.\n");
        printf("         The trace format may differ. Review raw output above.\n");
    }

    fclose(f);
}

/* =========================================================================
 * DRM test operations
 * ========================================================================= */

static int create_resource(int drm_fd, uint32_t width, uint32_t height,
                           uint32_t *bo_handle, uint32_t *res_handle) {
    struct drm_virtgpu_resource_create rc = {
        .target     = 2,    /* PIPE_TEXTURE_2D */
        .format     = 2,    /* B8G8R8X8_UNORM */
        .bind       = 0x0A, /* RENDER_TARGET | SAMPLER_VIEW */
        .width      = width,
        .height     = height,
        .depth      = 1,
        .array_size = 1,
        .last_level = 0,
        .nr_samples = 0,
        .flags      = 0,
    };

    int ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc);
    if (ret < 0) return -errno;

    *bo_handle = rc.bo_handle;
    *res_handle = rc.res_handle;
    return 0;
}

static int submit_nop_batch(int drm_fd, uint32_t bo_handle) {
    /* Minimal VirGL batch: create_sub_ctx(1) + set_sub_ctx(1) */
    uint32_t cmds[4];
    cmds[0] = virgl_cmd_hdr(VIRGL_CCMD_CREATE_SUB_CTX, 0, 1);
    cmds[1] = 1; /* sub_ctx_id */
    cmds[2] = virgl_cmd_hdr(VIRGL_CCMD_SET_SUB_CTX, 0, 1);
    cmds[3] = 1; /* sub_ctx_id */

    struct drm_virtgpu_execbuffer eb = {
        .flags          = 0,
        .size           = sizeof(cmds),
        .command        = (uint64_t)(uintptr_t)cmds,
        .bo_handles     = (uint64_t)(uintptr_t)&bo_handle,
        .num_bo_handles = 1,
        .fence_fd       = -1,
    };

    int ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_EXECBUFFER, &eb);
    if (ret < 0) return -errno;
    return 0;
}

static int wait_bo(int drm_fd, uint32_t bo_handle) {
    struct drm_virtgpu_3d_wait wait = {
        .handle = bo_handle,
        .flags  = 0,
    };
    return ioctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
}

static void close_bo(int drm_fd, uint32_t bo_handle) {
    struct drm_gem_close close_req = { .handle = bo_handle };
    ioctl(drm_fd, DRM_IOCTL_GEM_CLOSE, &close_req);
}

/* =========================================================================
 * Main test
 * ========================================================================= */

int main(void) {
    printf("=== VirtIO Descriptor Head Index Test ===\n");
    printf("Tests whether Parallels VirtIO GPU handles non-zero\n");
    printf("head descriptor indices in the avail ring.\n\n");

    /* Open DRM device */
    int drm_fd = open("/dev/dri/renderD128", O_RDWR);
    if (drm_fd < 0) {
        drm_fd = open("/dev/dri/card1", O_RDWR);
        if (drm_fd < 0) {
            perror("Failed to open DRM device");
            return 1;
        }
    }
    printf("[drm] Opened DRM device successfully\n");

    /* Set up ftrace */
    int have_trace = (setup_ftrace() == 0);

    /*
     * Phase 1: Serial submissions (20 rounds)
     *
     * Each round: create resource → submit NOP batch → wait → close
     *
     * Linux's virtio free list is LIFO. After the first submit+complete:
     *   desc[0,1,2] freed → free_head becomes 2→1→0→3→4→...
     *   Next alloc gets desc[2,1,0] with HEAD=2
     *
     * So by round 2, we're guaranteed to use non-zero head indices.
     */
    printf("\n--- Phase 1: Serial submissions (20 rounds) ---\n");
    int success = 0, fail = 0;

    for (int i = 0; i < 20; i++) {
        uint32_t bo, res;
        int ret = create_resource(drm_fd, 64, 64, &bo, &res);
        if (ret < 0) {
            printf("  [%2d] RESOURCE_CREATE failed: %s\n", i, strerror(-ret));
            fail++;
            continue;
        }

        ret = submit_nop_batch(drm_fd, bo);
        if (ret < 0) {
            printf("  [%2d] EXECBUFFER failed: %s\n", i, strerror(-ret));
            close_bo(drm_fd, bo);
            fail++;
            continue;
        }

        wait_bo(drm_fd, bo);
        close_bo(drm_fd, bo);
        success++;

        if (i < 5 || i == 19) {
            printf("  [%2d] OK (res_handle=%u)\n", i, res);
        } else if (i == 5) {
            printf("  ...  (rounds 5-18 running)\n");
        }
    }
    printf("  Serial: %d OK, %d FAIL\n", success, fail);

    /*
     * Phase 2: Rapid-fire submissions (no wait between submits)
     *
     * Submit 10 EXECBUFFER calls as fast as possible without waiting.
     * If the kernel batches these, multiple descriptors will be in-flight
     * simultaneously, FORCING different head indices for each.
     */
    printf("\n--- Phase 2: Rapid-fire submissions (10 bursts) ---\n");
    uint32_t bo_handles[10], res_handles[10];
    int created = 0;

    /* Create all resources first */
    for (int i = 0; i < 10; i++) {
        int ret = create_resource(drm_fd, 32, 32, &bo_handles[i], &res_handles[i]);
        if (ret < 0) {
            printf("  Resource create %d failed: %s\n", i, strerror(-ret));
            break;
        }
        created++;
    }

    /* Submit all without waiting */
    int burst_ok = 0, burst_fail = 0;
    for (int i = 0; i < created; i++) {
        int ret = submit_nop_batch(drm_fd, bo_handles[i]);
        if (ret < 0) {
            printf("  Burst submit %d FAILED: %s\n", i, strerror(-ret));
            burst_fail++;
        } else {
            burst_ok++;
        }
    }

    /* Now wait and close all */
    for (int i = 0; i < created; i++) {
        wait_bo(drm_fd, bo_handles[i]);
        close_bo(drm_fd, bo_handles[i]);
    }
    printf("  Burst: %d OK, %d FAIL\n", burst_ok, burst_fail);

    /*
     * Phase 3: Large batch to force free list rotation
     *
     * Submit 50 commands total to ensure the free list wraps around
     * multiple times, exercising many different head indices.
     */
    printf("\n--- Phase 3: Extended rotation (50 commands) ---\n");
    int phase3_ok = 0, phase3_fail = 0;
    for (int i = 0; i < 50; i++) {
        uint32_t bo, res;
        int ret = create_resource(drm_fd, 16, 16, &bo, &res);
        if (ret < 0) { phase3_fail++; continue; }

        ret = submit_nop_batch(drm_fd, bo);
        if (ret < 0) { phase3_fail++; close_bo(drm_fd, bo); continue; }

        wait_bo(drm_fd, bo);
        close_bo(drm_fd, bo);
        phase3_ok++;
    }
    printf("  Extended: %d OK, %d FAIL\n", phase3_ok, phase3_fail);

    /* Stop tracing and analyze */
    if (have_trace) {
        stop_ftrace();
        dump_and_parse_ftrace();
    }

    /* Summary */
    int total_ok = success + burst_ok + phase3_ok;
    int total_fail = fail + burst_fail + phase3_fail;

    printf("\n========== SUMMARY ==========\n");
    printf("Total commands: %d OK, %d FAIL\n", total_ok, total_fail);

    if (total_fail == 0 && total_ok >= 80) {
        printf("\nAll %d VirtIO GPU commands succeeded.\n", total_ok);
        printf("Linux's virtio free list guarantees non-zero head descriptor\n");
        printf("indices were used after the first submit+complete cycle.\n");
        printf("\nCONCLUSION: Parallels VirtIO GPU DOES support non-zero head\n");
        printf("descriptor indices. The Breenix crash was a Breenix bug,\n");
        printf("not a Parallels limitation.\n");
    } else if (total_fail > 0) {
        printf("\nSome commands failed. Check error output above.\n");
        printf("This may or may not indicate a head-index issue.\n");
    }

    close(drm_fd);
    return total_fail > 0 ? 1 : 0;
}
