/*
 * blue_mmap_test.c — Minimal "write blue, see blue" test via DRM + GEM mmap.
 *
 * Validates that CPU-written pixels through a page-aligned GEM buffer
 * can be displayed. This is the Linux-side equivalent of the Breenix
 * heap-allocated backing test that showed BLUE on Parallels.
 *
 * Build:  gcc -O2 -I/usr/include/libdrm -o blue_mmap_test blue_mmap_test.c -ldrm
 * Run:    sudo ./blue_mmap_test
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

/* VirtGPU MAP ioctl — maps a GEM BO into userspace */
struct drm_virtgpu_map {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;  /* output: mmap offset */
};

#define DRM_VIRTGPU_MAP 0x01
#define DRM_IOCTL_VIRTGPU_MAP \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_MAP, struct drm_virtgpu_map)

/* VirtGPU 3D resource create */
struct drm_virtgpu_resource_create {
    uint32_t target, format, bind, width, height, depth;
    uint32_t array_size, last_level, nr_samples, flags;
    uint32_t bo_handle, res_handle, size, stride;
};

#define DRM_VIRTGPU_RESOURCE_CREATE 0x04
#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE, \
             struct drm_virtgpu_resource_create)

/* VirtGPU transfer to host */
struct drm_virtgpu_3d_transfer_to_host {
    uint32_t bo_handle, pad;
    uint64_t offset;
    uint32_t level, stride, layer_stride;
    struct { uint32_t x, y, z, w, h, d; } box;
};

#define DRM_VIRTGPU_TRANSFER_TO_HOST 0x07
#define DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_TRANSFER_TO_HOST, \
             struct drm_virtgpu_3d_transfer_to_host)

/* VirtGPU wait */
struct drm_virtgpu_3d_wait {
    uint32_t handle, flags;
};

#define DRM_VIRTGPU_WAIT 0x08
#define DRM_IOCTL_VIRTGPU_WAIT \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_WAIT, struct drm_virtgpu_3d_wait)

/* Pipe constants */
#define PIPE_TEXTURE_2D            2
#define PIPE_FORMAT_B8G8R8X8_UNORM 2
#define PIPE_BIND_RENDER_TARGET    0x002
#define PIPE_BIND_SAMPLER_VIEW     0x008
#define PIPE_BIND_SCANOUT          0x40000
#define PIPE_BIND_SHARED           0x100000

int main(void)
{
    printf("=== Blue mmap Test — CPU-fill GEM buffer, display via DRM ===\n\n");

    /* Find DRM device (virtio_gpu) */
    const char *cards[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};
    int fd = -1;
    uint32_t conn_id = 0, crtc_id = 0;
    drmModeModeInfo mode;
    drmModeCrtcPtr saved_crtc = NULL;

    for (int i = 0; cards[i]; i++) {
        fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (fd < 0) continue;
        if (drmSetMaster(fd) < 0) { close(fd); fd = -1; continue; }

        drmModeResPtr res = drmModeGetResources(fd);
        if (!res) { close(fd); fd = -1; continue; }

        drmModeConnectorPtr conn = NULL;
        for (int c = 0; c < res->count_connectors; c++) {
            conn = drmModeGetConnector(fd, res->connectors[c]);
            if (conn && conn->connection == DRM_MODE_CONNECTED && conn->count_modes > 0)
                break;
            if (conn) drmModeFreeConnector(conn);
            conn = NULL;
        }
        if (!conn) { drmModeFreeResources(res); close(fd); fd = -1; continue; }

        conn_id = conn->connector_id;
        mode = conn->modes[0];

        drmModeEncoderPtr enc = conn->encoder_id ? drmModeGetEncoder(fd, conn->encoder_id) : NULL;
        if (!enc && res->count_encoders > 0) enc = drmModeGetEncoder(fd, res->encoders[0]);
        crtc_id = enc ? enc->crtc_id : (res->count_crtcs > 0 ? res->crtcs[0] : 0);
        if (enc) drmModeFreeEncoder(enc);
        if (!crtc_id && res->count_crtcs > 0) crtc_id = res->crtcs[0];

        saved_crtc = drmModeGetCrtc(fd, crtc_id);
        printf("DRM: %s — %ux%u, conn=%u, crtc=%u\n", cards[i],
               mode.hdisplay, mode.vdisplay, conn_id, crtc_id);

        drmModeFreeConnector(conn);
        drmModeFreeResources(res);
        break;
    }

    if (fd < 0) { fprintf(stderr, "No DRM device found\n"); return 1; }

    uint32_t W = mode.hdisplay, H = mode.vdisplay;

    /* ---- Approach 1: 3D resource + GEM mmap ---- */
    printf("\n--- Approach 1: VIRTGPU_RESOURCE_CREATE (3D) + mmap ---\n");
    {
        struct drm_virtgpu_resource_create rc = {0};
        rc.target = PIPE_TEXTURE_2D;
        rc.format = PIPE_FORMAT_B8G8R8X8_UNORM;
        rc.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW |
                  PIPE_BIND_SCANOUT | PIPE_BIND_SHARED;
        rc.width = W; rc.height = H; rc.depth = 1; rc.array_size = 1;

        if (drmIoctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc) < 0) {
            printf("  RESOURCE_CREATE failed: %s\n", strerror(errno));
        } else {
            printf("  resource: bo=%u, res=%u, size=%u, stride=%u\n",
                   rc.bo_handle, rc.res_handle, rc.size, rc.stride);

            /* mmap the GEM BO */
            struct drm_virtgpu_map vmap = {0};
            vmap.handle = rc.bo_handle;
            if (drmIoctl(fd, DRM_IOCTL_VIRTGPU_MAP, &vmap) < 0) {
                printf("  VIRTGPU_MAP failed: %s\n", strerror(errno));
            } else {
                printf("  mmap offset: 0x%lx\n", (unsigned long)vmap.offset);
                uint32_t *pixels = mmap(NULL, rc.size, PROT_READ | PROT_WRITE,
                                        MAP_SHARED, fd, vmap.offset);
                if (pixels == MAP_FAILED) {
                    printf("  mmap failed: %s\n", strerror(errno));
                } else {
                    /* Fill with cornflower blue (B8G8R8X8: B=0xED, G=0x95, R=0x64) */
                    uint32_t blue = 0x00ED9564; /* BGRA: cornflower blue */
                    printf("  filling %ux%u with cornflower blue (0x%08x)...\n", W, H, blue);
                    for (uint32_t i = 0; i < W * H; i++)
                        pixels[i] = blue;
                    printf("  fill complete (%u pixels)\n", W * H);

                    /* Try TRANSFER_TO_HOST_3D */
                    struct drm_virtgpu_3d_transfer_to_host xfer = {0};
                    xfer.bo_handle = rc.bo_handle;
                    xfer.stride = rc.stride ? rc.stride : W * 4;
                    xfer.box.w = W; xfer.box.h = H; xfer.box.d = 1;
                    int tr = drmIoctl(fd, DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST, &xfer);
                    printf("  TRANSFER_TO_HOST: %s\n", tr < 0 ? strerror(errno) : "OK");

                    /* Wait */
                    struct drm_virtgpu_3d_wait wait = { .handle = rc.bo_handle };
                    int wr = drmIoctl(fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
                    printf("  WAIT: %s\n", wr < 0 ? strerror(errno) : "OK");

                    /* AddFB + SetCrtc */
                    uint32_t stride = rc.stride ? rc.stride : W * 4;
                    uint32_t fb_id = 0;
                    if (drmModeAddFB(fd, W, H, 24, 32, stride, rc.bo_handle, &fb_id) < 0) {
                        printf("  AddFB failed: %s\n", strerror(errno));
                    } else {
                        printf("  AddFB: fb=%u\n", fb_id);
                        if (drmModeSetCrtc(fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode) < 0)
                            printf("  SetCrtc failed: %s\n", strerror(errno));
                        else
                            printf("  SetCrtc: OK\n");

                        /* Try DirtyFB to trigger display update */
                        drmModeClip clip = { .x1 = 0, .y1 = 0, .x2 = W, .y2 = H };
                        int dr = drmModeDirtyFB(fd, fb_id, &clip, 1);
                        printf("  DirtyFB: %s\n", dr < 0 ? strerror(errno) : "OK");
                    }

                    munmap(pixels, rc.size);
                }
            }
        }
    }

    /* ---- Approach 2: dumb buffer (no 3D, kernel-managed) ---- */
    printf("\n--- Approach 2: DRM_IOCTL_MODE_CREATE_DUMB (2D) + mmap ---\n");
    {
        struct drm_mode_create_dumb create = {0};
        create.width = W;
        create.height = H;
        create.bpp = 32;
        if (drmIoctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &create) < 0) {
            printf("  CREATE_DUMB failed: %s\n", strerror(errno));
        } else {
            printf("  dumb: handle=%u, size=%llu, stride=%u\n",
                   create.handle, (unsigned long long)create.size, create.pitch);

            /* Map dumb buffer */
            struct drm_mode_map_dumb mreq = {0};
            mreq.handle = create.handle;
            if (drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mreq) < 0) {
                printf("  MAP_DUMB failed: %s\n", strerror(errno));
            } else {
                uint32_t *pixels = mmap(NULL, create.size, PROT_READ | PROT_WRITE,
                                        MAP_SHARED, fd, mreq.offset);
                if (pixels == MAP_FAILED) {
                    printf("  mmap failed: %s\n", strerror(errno));
                } else {
                    /* Fill with pure blue (B8G8R8X8: B=0xFF, G=0, R=0) */
                    uint32_t blue = 0x00FF0000; /* pure blue in BGRA */
                    printf("  filling %ux%u with pure blue (0x%08x)...\n", W, H, blue);
                    for (uint32_t i = 0; i < W * H; i++)
                        pixels[i] = blue;
                    printf("  fill complete\n");

                    /* AddFB + SetCrtc */
                    uint32_t fb_id = 0;
                    if (drmModeAddFB(fd, W, H, 24, 32, create.pitch, create.handle, &fb_id) < 0) {
                        printf("  AddFB failed: %s\n", strerror(errno));
                    } else {
                        printf("  AddFB: fb=%u\n", fb_id);
                        if (drmModeSetCrtc(fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode) < 0)
                            printf("  SetCrtc failed: %s\n", strerror(errno));
                        else
                            printf("  SetCrtc: OK\n");

                        drmModeClip clip = { .x1 = 0, .y1 = 0, .x2 = W, .y2 = H };
                        int dr = drmModeDirtyFB(fd, fb_id, &clip, 1);
                        printf("  DirtyFB: %s\n", dr < 0 ? strerror(errno) : "OK");
                    }

                    munmap(pixels, create.size);
                }
            }
        }
    }

    printf("\n=== Holding for 10 seconds — check display NOW ===\n");
    fflush(stdout);
    sleep(10);

    /* Restore */
    if (saved_crtc) {
        drmModeSetCrtc(fd, saved_crtc->crtc_id, saved_crtc->buffer_id,
                       saved_crtc->x, saved_crtc->y, &conn_id, 1, &saved_crtc->mode);
        drmModeFreeCrtc(saved_crtc);
    }
    close(fd);
    printf("Done.\n");
    return 0;
}
