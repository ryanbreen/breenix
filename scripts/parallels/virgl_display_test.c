/*
 * virgl_display_test.c - VirGL CLEAR + readback verification + DRM display
 *
 * Creates a 3D resource, does VirGL CLEAR to GREEN via EXECBUFFER,
 * then reads back via TRANSFER_FROM_HOST + MAP to verify pixels,
 * then displays via DRM AddFB + SetCrtc.
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
#include <drm/drm.h>
#include <drm/virtgpu_drm.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

#define VIRGL_CMD0(cmd, obj, len) ((cmd) | ((obj) << 8) | ((uint32_t)(len) << 16))

#define CCMD_CREATE_OBJECT      1
#define CCMD_SET_FRAMEBUFFER_STATE 5
#define CCMD_CLEAR              7
#define CCMD_SET_SUB_CTX        28
#define CCMD_CREATE_SUB_CTX     29
#define CCMD_SET_TWEAKS         46

#define OBJ_SURFACE     8

#define PIPE_CLEAR_COLOR0    0x04
#define PIPE_TEXTURE_2D      2
#define VIRGL_FORMAT_B8G8R8X8_UNORM 2
#define PIPE_BIND_RT         (1 << 1)
#define PIPE_BIND_SV         (1 << 3)
#define PIPE_BIND_SCANOUT    (1 << 14)

static int drm_fd = -1;
static uint32_t cmdbuf[8192];
static int cmdpos = 0;
static uint32_t conn_id, crtc_id;
static drmModeModeInfo mode;

static void cmd_push(uint32_t val) { cmdbuf[cmdpos++] = val; }
static union { float f; uint32_t u; } f2u_helper;
static uint32_t f2u(float f) { f2u_helper.f = f; return f2u_helper.u; }

static int find_drm_display(void) {
    const char *cards[] = {"/dev/dri/card1", "/dev/dri/card0", NULL};
    for (int i = 0; cards[i]; i++) {
        int fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (fd < 0) { printf("open(%s) failed: %s\n", cards[i], strerror(errno)); continue; }

        uint64_t has_3d = 0;
        struct drm_virtgpu_getparam gp = { .param = 1, .value = (uint64_t)(uintptr_t)&has_3d };
        if (ioctl(fd, DRM_IOCTL_VIRTGPU_GETPARAM, &gp) < 0 || !has_3d) {
            printf("%s: not virtgpu 3D (err=%s, val=%llu)\n", cards[i], strerror(errno), (unsigned long long)has_3d);
            close(fd); continue;
        }
        printf("%s: virtgpu 3D OK (val=%llu)\n", cards[i], (unsigned long long)has_3d);

        if (drmSetMaster(fd) < 0) { printf("%s: drmSetMaster failed: %s\n", cards[i], strerror(errno)); close(fd); continue; }
        printf("%s: drmSetMaster OK\n", cards[i]);

        drmModeResPtr res = drmModeGetResources(fd);
        if (!res) { close(fd); continue; }

        drmModeConnectorPtr conn = NULL;
        for (int c = 0; c < res->count_connectors; c++) {
            conn = drmModeGetConnector(fd, res->connectors[c]);
            if (conn && conn->connection == DRM_MODE_CONNECTED && conn->count_modes > 0) break;
            if (conn) drmModeFreeConnector(conn);
            conn = NULL;
        }
        if (!conn) { drmModeFreeResources(res); close(fd); continue; }

        conn_id = conn->connector_id;
        mode = conn->modes[0];
        drmModeEncoderPtr enc = conn->encoder_id ? drmModeGetEncoder(fd, conn->encoder_id) : NULL;
        crtc_id = enc ? enc->crtc_id : res->crtcs[0];
        if (enc) drmModeFreeEncoder(enc);
        if (!crtc_id && res->count_crtcs > 0) crtc_id = res->crtcs[0];

        printf("DRM: %s -- %ux%u, conn=%u, crtc=%u\n",
               cards[i], mode.hdisplay, mode.vdisplay, conn_id, crtc_id);
        drmModeFreeConnector(conn);
        drmModeFreeResources(res);
        drm_fd = fd;
        return 0;
    }
    return -1;
}

int main(void) {
    printf("=== VirGL Display Test (CLEAR + readback + DRM SetCrtc) ===\n\n");

    if (find_drm_display() < 0) { fprintf(stderr, "No DRM device\n"); return 1; }

    uint32_t W = mode.hdisplay, H = mode.vdisplay;

    /* Create 3D resource */
    struct drm_virtgpu_resource_create rc = {
        .target = PIPE_TEXTURE_2D,
        .format = VIRGL_FORMAT_B8G8R8X8_UNORM,
        .bind = PIPE_BIND_RT | PIPE_BIND_SV | PIPE_BIND_SCANOUT,
        .width = W, .height = H, .depth = 1,
        .array_size = 1, .last_level = 0, .nr_samples = 0,
        .size = W * H * 4,
    };
    if (ioctl(drm_fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &rc) < 0) {
        perror("RESOURCE_CREATE"); return 1;
    }
    printf("Resource: res=%u bo=%u (%ux%u)\n", rc.res_handle, rc.bo_handle, W, H);

    /* Build VirGL commands */
    cmdpos = 0;

    /* Sub-context */
    cmd_push(VIRGL_CMD0(CCMD_CREATE_SUB_CTX, 0, 1));
    cmd_push(1);
    cmd_push(VIRGL_CMD0(CCMD_SET_SUB_CTX, 0, 1));
    cmd_push(1);

    /* Tweaks */
    cmd_push(VIRGL_CMD0(CCMD_SET_TWEAKS, 0, 2));
    cmd_push(1); cmd_push(1);
    cmd_push(VIRGL_CMD0(CCMD_SET_TWEAKS, 0, 2));
    cmd_push(2); cmd_push(W);

    /* Create surface */
    cmd_push(VIRGL_CMD0(CCMD_CREATE_OBJECT, OBJ_SURFACE, 5));
    cmd_push(1); /* handle */
    cmd_push(rc.res_handle);
    cmd_push(VIRGL_FORMAT_B8G8R8X8_UNORM);
    cmd_push(0); cmd_push(0);

    /* Set framebuffer */
    cmd_push(VIRGL_CMD0(CCMD_SET_FRAMEBUFFER_STATE, 0, 3));
    cmd_push(1); /* nr_cbufs */
    cmd_push(0); /* zsurf */
    cmd_push(1); /* cbuf[0] = surface 1 */

    /* CLEAR to GREEN (R=0, G=1, B=0, A=1) */
    cmd_push(VIRGL_CMD0(CCMD_CLEAR, 0, 8));
    cmd_push(PIPE_CLEAR_COLOR0);
    cmd_push(f2u(0.0f)); /* R */
    cmd_push(f2u(1.0f)); /* G */
    cmd_push(f2u(0.0f)); /* B */
    cmd_push(f2u(1.0f)); /* A */
    cmd_push(0x00000000); cmd_push(0x3FF00000); /* depth = 1.0 */
    cmd_push(0); /* stencil */

    printf("VirGL: %d dwords (%d bytes)\n", cmdpos, cmdpos * 4);

    /* Print hex dump of command buffer */
    printf("Cmd hex:");
    for (int i = 0; i < cmdpos; i++) {
        if (i % 8 == 0) printf("\n  [%02d]", i);
        printf(" %08x", cmdbuf[i]);
    }
    printf("\n\n");

    /* Submit EXECBUFFER */
    struct drm_virtgpu_execbuffer eb = {
        .size = cmdpos * 4,
        .command = (uint64_t)(uintptr_t)cmdbuf,
        .bo_handles = (uint64_t)(uintptr_t)&rc.bo_handle,
        .num_bo_handles = 1,
        .fence_fd = -1,
    };
    int ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_EXECBUFFER, &eb);
    printf("EXECBUFFER: ret=%d errno=%d (%s)\n", ret, ret < 0 ? errno : 0, ret < 0 ? strerror(errno) : "OK");
    if (ret < 0) return 1;

    /* Wait for GPU */
    struct drm_virtgpu_3d_wait wait = { .handle = rc.bo_handle };
    ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
    printf("GPU wait: ret=%d errno=%d (%s)\n", ret, ret < 0 ? errno : 0, ret < 0 ? strerror(errno) : "OK");

    /* Check dmesg for errors */
    printf("\nChecking dmesg for GPU errors...\n");
    fflush(stdout);
    system("dmesg | grep -i 'ERROR.*virtio_gpu' | tail -5");
    printf("\n");

    /* Readback: TRANSFER_FROM_HOST + MAP */
    printf("=== Readback: TRANSFER_FROM_HOST ===\n");
    struct drm_virtgpu_3d_transfer_from_host xfer = {
        .bo_handle = rc.bo_handle,
        .box = { .x = 0, .y = 0, .z = 0, .w = W, .h = H, .d = 1 },
        .level = 0, .offset = 0,
    };
    ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_FROM_HOST, &xfer);
    printf("TRANSFER_FROM_HOST: ret=%d errno=%d (%s)\n", ret, ret < 0 ? errno : 0, ret < 0 ? strerror(errno) : "OK");

    /* Wait again */
    ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);

    /* MAP the BO */
    struct drm_virtgpu_map map_info = { .handle = rc.bo_handle };
    ret = ioctl(drm_fd, DRM_IOCTL_VIRTGPU_MAP, &map_info);
    printf("MAP: ret=%d offset=0x%llx\n", ret, (unsigned long long)map_info.offset);

    if (ret == 0) {
        uint32_t *pixels = mmap(NULL, W * H * 4, PROT_READ, MAP_SHARED, drm_fd, map_info.offset);
        if (pixels != MAP_FAILED) {
            printf("Pixel readback (first 16 pixels):\n");
            for (int i = 0; i < 16 && i < (int)(W * H); i++) {
                uint32_t p = pixels[i];
                printf("  [%d] 0x%08x (B=%d G=%d R=%d A=%d)\n", i, p,
                       p & 0xFF, (p >> 8) & 0xFF, (p >> 16) & 0xFF, (p >> 24) & 0xFF);
            }
            /* Check center pixel */
            uint32_t center = pixels[(H/2) * W + W/2];
            printf("\nCenter pixel: 0x%08x (B=%d G=%d R=%d A=%d)\n", center,
                   center & 0xFF, (center >> 8) & 0xFF, (center >> 16) & 0xFF, (center >> 24) & 0xFF);

            /* Count non-zero pixels */
            int nonzero = 0;
            for (uint32_t y = 0; y < H; y += H/4) {
                for (uint32_t x = 0; x < W; x += W/4) {
                    uint32_t p = pixels[y * W + x];
                    if (p != 0) nonzero++;
                    printf("  Sample (%u,%u): 0x%08x\n", x, y, p);
                }
            }
            printf("Non-zero samples: %d\n", nonzero);

            if (center == 0x0000FF00 || center == 0xFF00FF00) {
                printf("\n*** GREEN PIXELS CONFIRMED - VirGL CLEAR WORKS ***\n");
            } else if (center == 0) {
                printf("\n*** ALL BLACK - VirGL CLEAR DID NOT WRITE PIXELS ***\n");
            } else {
                printf("\n*** UNEXPECTED COLOR - check encoding ***\n");
            }

            munmap(pixels, W * H * 4);
        } else {
            perror("mmap");
        }
    }

    /* Display via DRM */
    printf("\n=== DRM Display ===\n");
    uint32_t fb_id = 0;
    uint32_t stride = W * 4;
    if (drmModeAddFB(drm_fd, W, H, 24, 32, stride, rc.bo_handle, &fb_id) < 0) {
        printf("AddFB failed: %s\n", strerror(errno));
        uint32_t handles[4] = {rc.bo_handle, 0, 0, 0};
        uint32_t strides[4] = {stride, 0, 0, 0};
        uint32_t offsets[4] = {0, 0, 0, 0};
        if (drmModeAddFB2(drm_fd, W, H, 0x34325258, handles, strides, offsets, &fb_id, 0) < 0) {
            perror("AddFB2"); return 1;
        }
    }
    printf("AddFB: fb_id=%u\n", fb_id);

    if (drmModeSetCrtc(drm_fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode) < 0) {
        perror("SetCrtc"); return 1;
    }
    printf("SetCrtc: OK\n");

    printf("\nHolding for 30 seconds...\n");
    fflush(stdout);
    sleep(30);

    printf("Done.\n");
    close(drm_fd);
    return 0;
}
