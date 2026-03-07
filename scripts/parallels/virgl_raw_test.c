/*
 * virgl_raw_test.c — Standalone VirGL reference program for byte comparison
 *
 * Sends hand-crafted VirGL commands via raw DRM ioctls (no Mesa/EGL/GBM).
 * Every DWORD of every VirGL batch is hex-dumped before submission so it can
 * be diffed byte-for-byte against Breenix serial output.
 *
 * Build:  gcc -O2 -o virgl_raw_test virgl_raw_test.c -ldrm
 * Run:    sudo ./virgl_raw_test
 *
 * Expected result: cornflower blue screen for 5 seconds.
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
#include <sys/select.h>
#include <math.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

/* =========================================================================
 * VirtGPU DRM ioctl definitions (from linux/virtgpu_drm.h)
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

#define DRM_VIRTGPU_MAP              0x01
#define DRM_VIRTGPU_EXECBUFFER       0x02
#define DRM_VIRTGPU_RESOURCE_CREATE  0x04
#define DRM_VIRTGPU_TRANSFER_FROM_HOST 0x06
#define DRM_VIRTGPU_TRANSFER_TO_HOST 0x07
#define DRM_VIRTGPU_WAIT             0x08

struct drm_virtgpu_map {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;  /* output: mmap offset */
};

struct drm_virtgpu_3d_transfer_to_host {
    uint32_t bo_handle;
    uint32_t pad;
    uint64_t offset;
    uint32_t level;
    uint32_t stride;
    uint32_t layer_stride;
    struct {
        uint32_t x, y, z, w, h, d;
    } box;
};

/* TRANSFER_FROM_HOST uses the same struct layout */
typedef struct drm_virtgpu_3d_transfer_to_host drm_virtgpu_3d_transfer_from_host;

struct drm_virtgpu_3d_wait {
    uint32_t handle;
    uint32_t flags;
};

#define DRM_IOCTL_VIRTGPU_MAP \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_MAP, \
             struct drm_virtgpu_map)

#define DRM_IOCTL_VIRTGPU_EXECBUFFER \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_EXECBUFFER, \
             struct drm_virtgpu_execbuffer)

#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE, \
             struct drm_virtgpu_resource_create)

#define DRM_IOCTL_VIRTGPU_TRANSFER_FROM_HOST \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_TRANSFER_FROM_HOST, \
             drm_virtgpu_3d_transfer_from_host)

#define DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_TRANSFER_TO_HOST, \
             struct drm_virtgpu_3d_transfer_to_host)

#define DRM_IOCTL_VIRTGPU_WAIT \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_WAIT, \
             struct drm_virtgpu_3d_wait)

/* =========================================================================
 * VirGL constants — must match kernel/src/drivers/virtio/virgl.rs exactly
 * ========================================================================= */

/* Command types */
#define VIRGL_CCMD_NOP                  0
#define VIRGL_CCMD_CREATE_OBJECT        1
#define VIRGL_CCMD_BIND_OBJECT          2
#define VIRGL_CCMD_SET_VIEWPORT_STATE   4
#define VIRGL_CCMD_SET_FRAMEBUFFER_STATE 5
#define VIRGL_CCMD_SET_VERTEX_BUFFERS   6
#define VIRGL_CCMD_CLEAR                7
#define VIRGL_CCMD_DRAW_VBO             8
#define VIRGL_CCMD_RESOURCE_INLINE_WRITE 9
#define VIRGL_CCMD_SET_SCISSOR_STATE    15
#define VIRGL_CCMD_SET_SUB_CTX          28
#define VIRGL_CCMD_CREATE_SUB_CTX       29
#define VIRGL_CCMD_BIND_SHADER          31

/* Object types */
#define VIRGL_OBJ_BLEND           1
#define VIRGL_OBJ_RASTERIZER      2
#define VIRGL_OBJ_DSA             3
#define VIRGL_OBJ_SHADER          4
#define VIRGL_OBJ_VERTEX_ELEMENTS 5
#define VIRGL_OBJ_SURFACE         8

/* Pipe constants */
#define PIPE_BUFFER        0
#define PIPE_TEXTURE_2D    2
#define PIPE_PRIM_TRIANGLES 4

#define PIPE_FORMAT_B8G8R8X8_UNORM    2
#define PIPE_FORMAT_R8G8B8A8_UNORM    67
#define PIPE_FORMAT_R32G32B32A32_FLOAT 31

#define PIPE_BIND_RENDER_TARGET   0x002
#define PIPE_BIND_SAMPLER_VIEW    0x008
#define PIPE_BIND_VERTEX_BUFFER   0x010
#define PIPE_BIND_SCANOUT         0x40000
#define PIPE_BIND_SHARED          0x100000

#define PIPE_CLEAR_COLOR0  0x04

#define PIPE_SHADER_VERTEX   0
#define PIPE_SHADER_FRAGMENT 1

/* =========================================================================
 * VirGL command buffer builder
 * ========================================================================= */

#define CMD_BUF_MAX 4096

static uint32_t cmd_buf[CMD_BUF_MAX];
static int cmd_len;

static void cmd_reset(void) { cmd_len = 0; }

static void cmd_push(uint32_t v)
{
    if (cmd_len < CMD_BUF_MAX)
        cmd_buf[cmd_len++] = v;
}

static uint32_t cmd0(uint32_t cmd, uint32_t obj, uint32_t len)
{
    return cmd | (obj << 8) | (len << 16);
}

static uint32_t f32_bits(float f)
{
    uint32_t u;
    memcpy(&u, &f, 4);
    return u;
}

/* Pack TGSI text into DWORDs (little-endian, null-terminated, zero-padded).
 * Returns number of DWORDs pushed. */
static int push_tgsi_text(const char *text)
{
    int text_len = strlen(text) + 1; /* include null terminator */
    int text_dwords = (text_len + 3) / 4;
    for (int i = 0; i < text_dwords; i++) {
        uint32_t dw = 0;
        for (int b = 0; b < 4; b++) {
            int idx = i * 4 + b;
            if (idx < text_len)
                dw |= ((uint32_t)(unsigned char)text[idx]) << (b * 8);
        }
        cmd_push(dw);
    }
    return text_dwords;
}

/* -------------------------------------------------------------------------
 * VirGL command builders — identical encoding to virgl.rs
 * ------------------------------------------------------------------------- */

static void cmd_create_sub_ctx(uint32_t id)
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_SUB_CTX, 0, 1));
    cmd_push(id);
}

static void cmd_set_sub_ctx(uint32_t id)
{
    cmd_push(cmd0(VIRGL_CCMD_SET_SUB_CTX, 0, 1));
    cmd_push(id);
}

static void cmd_create_shader(uint32_t handle, uint32_t shader_type, const char *tgsi)
{
    int text_len = strlen(tgsi) + 1;
    int text_dwords = (text_len + 3) / 4;
    int payload_len = 5 + text_dwords;

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_SHADER, payload_len));
    cmd_push(handle);
    cmd_push(shader_type);
    cmd_push(text_len);     /* bit 31 clear = first/only chunk */
    cmd_push(0);            /* NUM_TOKENS (0 for text TGSI) */
    cmd_push(0);            /* num_so_outputs */
    push_tgsi_text(tgsi);
}

static void cmd_create_blend_simple(uint32_t handle)
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_BLEND, 11));
    cmd_push(handle);
    cmd_push(0x00000004);   /* S0: dither enabled (bit 2), matches Mesa */
    cmd_push(0);            /* S1: logicop_func */
    cmd_push(0x78000000);   /* S2[0]: colormask=0xF<<27 (VIRGL_OBJ_BLEND_S2_RT_COLORMASK), blend disabled */
    cmd_push(0);            /* S2[1] */
    cmd_push(0);            /* S2[2] */
    cmd_push(0);            /* S2[3] */
    cmd_push(0);            /* S2[4] */
    cmd_push(0);            /* S2[5] */
    cmd_push(0);            /* S2[6] */
    cmd_push(0);            /* S2[7] */
}

static void cmd_create_dsa_disabled(uint32_t handle)
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_DSA, 5));
    cmd_push(handle);
    cmd_push(0);            /* S0: depth/alpha test disabled */
    cmd_push(0);            /* S1: front stencil disabled */
    cmd_push(0);            /* S2: back stencil disabled */
    cmd_push(0);            /* alpha_ref = 0.0f */
}

static void cmd_create_rasterizer_default(uint32_t handle)
{
    /* Match Mesa exactly: 0x60008082
     * bit 1:  depth_clip_near
     * bit 7:  point_quad_rasterization
     * bit 15: front_ccw
     * bit 29: half_pixel_center
     * bit 30: bottom_edge_rule
     * NO scissor (bit 14) — Mesa doesn't enable it for simple clears
     */
    uint32_t s0 = (1 << 1) | (1 << 7) | (1 << 15) | (1 << 29) | (1 << 30);

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_RASTERIZER, 9));
    cmd_push(handle);
    cmd_push(s0);                   /* 0x60008082 — matches Mesa */
    cmd_push(f32_bits(1.0f));       /* point_size */
    cmd_push(0);                    /* sprite_coord_enable */
    cmd_push(0x0000FFFF);           /* S3: clip_plane_enable = all (matches Mesa) */
    cmd_push(f32_bits(1.0f));       /* line_width */
    cmd_push(0);                    /* offset_units */
    cmd_push(0);                    /* offset_scale */
    cmd_push(0);                    /* offset_clamp */
}

static void cmd_create_vertex_elements(uint32_t handle, int count,
    uint32_t offsets[], uint32_t divisors[],
    uint32_t vb_indices[], uint32_t formats[])
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_VERTEX_ELEMENTS,
                  4 * count + 1));
    cmd_push(handle);
    for (int i = 0; i < count; i++) {
        cmd_push(offsets[i]);
        cmd_push(divisors[i]);
        cmd_push(vb_indices[i]);
        cmd_push(formats[i]);
    }
}

static void cmd_bind_object(uint32_t handle, uint32_t obj_type)
{
    cmd_push(cmd0(VIRGL_CCMD_BIND_OBJECT, obj_type, 1));
    cmd_push(handle);
}

static void cmd_bind_shader(uint32_t handle, uint32_t shader_type)
{
    cmd_push(cmd0(VIRGL_CCMD_BIND_SHADER, 0, 2));
    cmd_push(handle);
    cmd_push(shader_type);
}

static void cmd_set_viewport(float width, float height)
{
    cmd_push(cmd0(VIRGL_CCMD_SET_VIEWPORT_STATE, 0, 7));
    cmd_push(0);                        /* start_slot */
    cmd_push(f32_bits(width / 2.0f));   /* scale_x */
    cmd_push(f32_bits(-height / 2.0f)); /* scale_y (neg for GL Y-up) */
    cmd_push(f32_bits(0.5f));           /* scale_z */
    cmd_push(f32_bits(width / 2.0f));   /* translate_x */
    cmd_push(f32_bits(height / 2.0f));  /* translate_y */
    cmd_push(f32_bits(0.5f));           /* translate_z */
}

static void cmd_set_scissor_state(uint32_t min_x, uint32_t min_y,
                                  uint32_t max_x, uint32_t max_y)
{
    cmd_push(cmd0(VIRGL_CCMD_SET_SCISSOR_STATE, 0, 3));
    cmd_push(0);                                              /* start_slot */
    cmd_push((min_x & 0xFFFF) | ((min_y & 0xFFFF) << 16));
    cmd_push((max_x & 0xFFFF) | ((max_y & 0xFFFF) << 16));
}

static void cmd_create_surface(uint32_t handle, uint32_t res_handle,
                               uint32_t fmt, uint32_t level, uint32_t layers)
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_SURFACE, 5));
    cmd_push(handle);
    cmd_push(res_handle);
    cmd_push(fmt);
    cmd_push(level);
    cmd_push(layers);   /* first_layer | (last_layer << 16) */
}

static void cmd_set_framebuffer_state(uint32_t zsurf_handle,
                                      int nr_cbufs, uint32_t cbuf_handles[])
{
    cmd_push(cmd0(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, nr_cbufs + 2));
    cmd_push(nr_cbufs);
    cmd_push(zsurf_handle);
    for (int i = 0; i < nr_cbufs; i++)
        cmd_push(cbuf_handles[i]);
}

static void cmd_clear_color(float r, float g, float b, float a)
{
    cmd_push(cmd0(VIRGL_CCMD_CLEAR, 0, 8));
    cmd_push(PIPE_CLEAR_COLOR0);    /* buffer flags */
    cmd_push(f32_bits(r));
    cmd_push(f32_bits(g));
    cmd_push(f32_bits(b));
    cmd_push(f32_bits(a));
    cmd_push(0);    /* depth f64 low */
    cmd_push(0);    /* depth f64 high */
    cmd_push(0);    /* stencil */
}

/* =========================================================================
 * Hex dump
 * ========================================================================= */

static void hex_dump_dwords(const char *label, const uint32_t *data, int count)
{
    printf("[hex-dump] %s (%d DWORDs, %d bytes):\n", label, count, count * 4);
    for (int i = 0; i < count; i++) {
        printf("[hex-dump] %s +%d: 0x%08X\n", label, i * 4, data[i]);
    }
    printf("[hex-dump] %s END\n\n", label);
}

static void hex_dump_resource_create(const char *label,
                                     const struct drm_virtgpu_resource_create *rc)
{
    printf("[hex-dump] %s:\n", label);
    printf("[hex-dump]   target     = 0x%08X (%u)\n", rc->target, rc->target);
    printf("[hex-dump]   format     = 0x%08X (%u)\n", rc->format, rc->format);
    printf("[hex-dump]   bind       = 0x%08X\n", rc->bind);
    printf("[hex-dump]   width      = %u\n", rc->width);
    printf("[hex-dump]   height     = %u\n", rc->height);
    printf("[hex-dump]   depth      = %u\n", rc->depth);
    printf("[hex-dump]   array_size = %u\n", rc->array_size);
    printf("[hex-dump]   last_level = %u\n", rc->last_level);
    printf("[hex-dump]   nr_samples = %u\n", rc->nr_samples);
    printf("[hex-dump]   flags      = 0x%08X\n", rc->flags);
    printf("[hex-dump]   bo_handle  = %u (output)\n", rc->bo_handle);
    printf("[hex-dump]   res_handle = %u (output)\n", rc->res_handle);
    printf("[hex-dump]   size       = %u (output)\n", rc->size);
    printf("[hex-dump]   stride     = %u (output)\n", rc->stride);
    printf("\n");
}

/* =========================================================================
 * DRM helpers
 * ========================================================================= */

static int drm_fd = -1;
static uint32_t conn_id, crtc_id;
static drmModeModeInfo mode;
static drmModeCrtcPtr saved_crtc;

static void page_flip_handler(int fd, unsigned int frame, unsigned int sec,
                              unsigned int usec, void *data)
{
    (void)fd;
    (void)frame;
    (void)sec;
    (void)usec;

    int *pending = (int *)data;
    *pending = 0;
}

static int drm_page_flip_and_wait(uint32_t fb_id)
{
    int pending = 1;
    int ret = drmModePageFlip(drm_fd, crtc_id, fb_id, DRM_MODE_PAGE_FLIP_EVENT,
                              &pending);
    if (ret < 0) {
        fprintf(stderr, "drmModePageFlip failed: %s\n", strerror(errno));
        return -1;
    }

    drmEventContext ev;
    memset(&ev, 0, sizeof(ev));
    ev.version = DRM_EVENT_CONTEXT_VERSION;
    ev.page_flip_handler = page_flip_handler;

    while (pending) {
        fd_set fds;
        struct timeval tv;

        FD_ZERO(&fds);
        FD_SET(drm_fd, &fds);
        tv.tv_sec = 1;
        tv.tv_usec = 0;

        ret = select(drm_fd + 1, &fds, NULL, NULL, &tv);
        if (ret < 0) {
            if (errno == EINTR)
                continue;
            fprintf(stderr, "select() failed waiting for page flip: %s\n",
                    strerror(errno));
            return -1;
        }
        if (ret == 0) {
            fprintf(stderr, "Page flip timed out\n");
            return -1;
        }
        if (FD_ISSET(drm_fd, &fds))
            drmHandleEvent(drm_fd, &ev);
    }

    return 0;
}

static int drm_mark_dirty(uint32_t fb_id, uint32_t width, uint32_t height)
{
    drmModeClip clip;
    memset(&clip, 0, sizeof(clip));
    clip.x1 = 0;
    clip.y1 = 0;
    clip.x2 = (uint16_t)width;
    clip.y2 = (uint16_t)height;

    int ret = drmModeDirtyFB(drm_fd, fb_id, &clip, 1);
    if (ret < 0) {
        fprintf(stderr, "drmModeDirtyFB failed: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

static int find_drm_device(void)
{
    const char *cards[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};

    for (int i = 0; cards[i]; i++) {
        int fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (fd < 0)
            continue;

        if (drmSetMaster(fd) < 0) {
            close(fd);
            continue;
        }

        drmModeResPtr res = drmModeGetResources(fd);
        if (!res) {
            close(fd);
            continue;
        }

        /* Find connected connector */
        drmModeConnectorPtr conn = NULL;
        for (int c = 0; c < res->count_connectors; c++) {
            conn = drmModeGetConnector(fd, res->connectors[c]);
            if (conn && conn->connection == DRM_MODE_CONNECTED &&
                conn->count_modes > 0) {
                break;
            }
            if (conn) drmModeFreeConnector(conn);
            conn = NULL;
        }

        if (!conn) {
            drmModeFreeResources(res);
            close(fd);
            continue;
        }

        conn_id = conn->connector_id;
        mode = conn->modes[0]; /* preferred mode */

        /* Find CRTC */
        drmModeEncoderPtr enc = NULL;
        if (conn->encoder_id)
            enc = drmModeGetEncoder(fd, conn->encoder_id);
        if (!enc && res->count_encoders > 0)
            enc = drmModeGetEncoder(fd, res->encoders[0]);

        if (enc) {
            crtc_id = enc->crtc_id;
            if (!crtc_id && res->count_crtcs > 0)
                crtc_id = res->crtcs[0];
            drmModeFreeEncoder(enc);
        } else if (res->count_crtcs > 0) {
            crtc_id = res->crtcs[0];
        }

        saved_crtc = drmModeGetCrtc(fd, crtc_id);

        printf("DRM: %s — %s %ux%u@%u\n", cards[i], conn->connector_type_id ? "connected" : "?",
               mode.hdisplay, mode.vdisplay, mode.vrefresh);
        printf("DRM: connector=%u, crtc=%u\n", conn_id, crtc_id);

        drmModeFreeConnector(conn);
        drmModeFreeResources(res);
        drm_fd = fd;
        return 0;
    }

    fprintf(stderr, "No DRM device found\n");
    return -1;
}

/* =========================================================================
 * VirtGPU resource + execbuffer wrappers
 * ========================================================================= */

static int virtgpu_resource_create(struct drm_virtgpu_resource_create *rc)
{
    int ret = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, rc);
    if (ret < 0) {
        fprintf(stderr, "RESOURCE_CREATE failed: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

static int virtgpu_execbuffer(uint32_t *cmds, int dword_count,
                              uint32_t *bo_handles, int num_bos)
{
    struct drm_virtgpu_execbuffer eb;
    memset(&eb, 0, sizeof(eb));
    eb.size = dword_count * 4;
    eb.command = (uint64_t)(uintptr_t)cmds;
    if (num_bos > 0) {
        eb.bo_handles = (uint64_t)(uintptr_t)bo_handles;
        eb.num_bo_handles = num_bos;
    }
    eb.fence_fd = -1;

    int ret = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_EXECBUFFER, &eb);
    if (ret < 0) {
        fprintf(stderr, "EXECBUFFER failed: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

/* =========================================================================
 * main
 * ========================================================================= */

int main(void)
{
    printf("=== VirGL Raw Test — byte-for-byte reference ===\n\n");

    /* Step 1: Find DRM device */
    if (find_drm_device() < 0)
        return 1;

    uint32_t width = mode.hdisplay;
    uint32_t height = mode.vdisplay;
    printf("Resolution: %ux%u\n\n", width, height);

    /* Step 2: Create 3D render target resource (matches Breenix RESOURCE_3D_ID) */
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

    printf("=== RESOURCE_CREATE_3D ===\n");
    hex_dump_resource_create("RESOURCE_CREATE_3D (render target)", &rc);

    if (virtgpu_resource_create(&rc) < 0)
        return 1;

    uint32_t bo_handle = rc.bo_handle;
    uint32_t res_handle = rc.res_handle;
    uint32_t stride = rc.stride;
    printf("Resource created: bo=%u res=%u stride=%u size=%u\n\n",
           bo_handle, res_handle, rc.size, stride);

    /* TGSI shader sources — byte-for-byte identical to Breenix */
    const char *vs_text =
        "VERT\n"
        "DCL IN[0], POSITION\n"
        "DCL IN[1], GENERIC[0]\n"
        "DCL OUT[0], POSITION\n"
        "DCL OUT[1], GENERIC[0]\n"
        "  0: MOV OUT[0], IN[0]\n"
        "  1: MOV OUT[1], IN[1]\n"
        "  2: END\n";

    const char *fs_text =
        "FRAG\n"
        "DCL IN[0], GENERIC[0], PERSPECTIVE\n"
        "DCL OUT[0], COLOR\n"
        "  0: MOV OUT[0], IN[0]\n"
        "  1: END\n";

    /* === Single batch matching Mesa's exact sequence ===
     * Mesa sends everything in one big EXECBUFFER:
     * 1. CREATE_SUB_CTX + SET_SUB_CTX
     * 2. SET_TWEAKS (Mesa sends these before anything else)
     * 3. CREATE_SHADER (VS + FS) — created but NOT bound before clear
     * 4. CREATE_SURFACE + SET_FRAMEBUFFER_STATE + CLEAR
     * 5. Then: create/bind DSA, bind shaders, blend, rasterizer, etc.
     * For a clear-only test, we just need steps 1-4.
     */
    printf("=== VirGL Single Batch (Mesa-style) ===\n");
    cmd_reset();

    /* 1. Sub-context */
    cmd_create_sub_ctx(1);
    cmd_set_sub_ctx(1);

    /* 2. SET_TWEAKS — Mesa sends these; might configure VirGL behavior */
    /* SET_TWEAKS(tweak_id=1, value=1) */
    cmd_push(cmd0(46, 0, 2)); /* VIRGL_CCMD_SET_TWEAKS=46 */
    cmd_push(1); /* tweak_id */
    cmd_push(1); /* value */
    /* SET_TWEAKS(tweak_id=2, value=width) */
    cmd_push(cmd0(46, 0, 2));
    cmd_push(2);
    cmd_push(width);

    /* 3. Create shaders (Mesa creates them before clear, though they're not bound) */
    cmd_create_shader(1, PIPE_SHADER_VERTEX, vs_text);
    cmd_create_shader(2, PIPE_SHADER_FRAGMENT, fs_text);

    /* 4. CREATE_SURFACE + SET_FRAMEBUFFER + CLEAR (this is what Mesa does before any binds) */
    cmd_create_surface(1, res_handle, PIPE_FORMAT_B8G8R8X8_UNORM, 0, 0);

    {
        uint32_t cbufs[] = {1};
        cmd_set_framebuffer_state(0, 1, cbufs);
    }

    /* CLEAR with bright GREEN — depth=1.0 as double (0x3FF00000:00000000) like Mesa */
    cmd_push(cmd0(VIRGL_CCMD_CLEAR, 0, 8));
    cmd_push(PIPE_CLEAR_COLOR0);         /* buffers = 0x04 */
    cmd_push(f32_bits(0.0f));            /* R */
    cmd_push(f32_bits(1.0f));            /* G */
    cmd_push(f32_bits(0.0f));            /* B */
    cmd_push(f32_bits(1.0f));            /* A */
    cmd_push(0x00000000);                /* depth f64 low */
    cmd_push(0x3FF00000);                /* depth f64 high = 1.0 (matches Mesa) */
    cmd_push(0);                         /* stencil */

    hex_dump_dwords("SINGLE_BATCH_MESA_STYLE", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, &bo_handle, 1) < 0)
        return 1;
    printf("Single batch submitted OK\n\n");

    /* === Wait for VirGL rendering to complete === */
    printf("=== VIRTGPU_WAIT (sync rendering) ===\n");
    {
        struct drm_virtgpu_3d_wait wait;
        memset(&wait, 0, sizeof(wait));
        wait.handle = bo_handle;
        wait.flags = 0;
        int wr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
        if (wr < 0)
            printf("VIRTGPU_WAIT failed: %s (continuing anyway)\n", strerror(errno));
        else
            printf("VIRTGPU_WAIT: OK — rendering complete\n");
    }

    /* If stride is 0, compute it (width * 4) */
    if (stride == 0)
        stride = width * 4;

    /* === MAP the resource into userspace === */
    printf("\n=== VIRTGPU_MAP + mmap (get backing store access) ===\n");
    uint32_t *pixels = NULL;
    uint32_t map_size = stride * height;
    {
        struct drm_virtgpu_map vmap;
        memset(&vmap, 0, sizeof(vmap));
        vmap.handle = bo_handle;
        int mr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_MAP, &vmap);
        if (mr < 0) {
            printf("VIRTGPU_MAP failed: %s\n", strerror(errno));
        } else {
            printf("VIRTGPU_MAP: offset=0x%lx\n", (unsigned long)vmap.offset);
            pixels = mmap(NULL, map_size, PROT_READ | PROT_WRITE,
                          MAP_SHARED, drm_fd, vmap.offset);
            if (pixels == MAP_FAILED) {
                printf("mmap failed: %s\n", strerror(errno));
                pixels = NULL;
            } else {
                printf("mmap: OK (%u bytes at %p)\n", map_size, (void *)pixels);
            }
        }
    }

    /* === Check guest backing BEFORE transfer (should be zeros/black) === */
    if (pixels) {
        printf("\n=== Guest backing BEFORE TRANSFER_FROM_HOST ===\n");
        printf("pixel[0,0] = 0x%08X (expect 0x00000000 = black/zeros)\n", pixels[0]);
        printf("pixel[1,0] = 0x%08X\n", pixels[1]);
        printf("pixel[center] = 0x%08X\n", pixels[(height/2) * (stride/4) + width/2]);
    }

    /* === TRANSFER_FROM_HOST_3D: pull VirGL-rendered content from host to guest === */
    printf("\n=== TRANSFER_FROM_HOST_3D (host→guest) ===\n");
    printf("VirGL renders on the HOST. Guest backing is zeros.\n");
    printf("We need TRANSFER_FROM_HOST to pull rendered pixels back.\n");
    {
        drm_virtgpu_3d_transfer_from_host xfer;
        memset(&xfer, 0, sizeof(xfer));
        xfer.bo_handle = bo_handle;
        xfer.stride = stride;
        xfer.box.w = width;
        xfer.box.h = height;
        xfer.box.d = 1;
        int tr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_FROM_HOST, &xfer);
        if (tr < 0)
            printf("TRANSFER_FROM_HOST: FAILED — %s\n", strerror(errno));
        else
            printf("TRANSFER_FROM_HOST: OK\n");
    }

    /* Wait again after transfer */
    {
        struct drm_virtgpu_3d_wait wait;
        memset(&wait, 0, sizeof(wait));
        wait.handle = bo_handle;
        int wr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
        printf("WAIT after FROM_HOST: %s\n", wr < 0 ? strerror(errno) : "OK");
    }

    /* === Check guest backing AFTER transfer (should have green pixels) === */
    if (pixels) {
        printf("\n=== Guest backing AFTER TRANSFER_FROM_HOST ===\n");
        uint32_t p0 = pixels[0];
        uint32_t pc = pixels[(height/2) * (stride/4) + width/2];
        printf("pixel[0,0] = 0x%08X\n", p0);
        printf("pixel[1,0] = 0x%08X\n", pixels[1]);
        printf("pixel[center] = 0x%08X\n", pc);

        /* Analyze: for GREEN clear in B8G8R8X8_UNORM, expect B=0,G=FF,R=0,X=FF → 0xFF00FF00
         * or B=0,G=FF,R=0,X=0 → 0x0000FF00 depending on alpha handling */
        if (p0 == 0x00000000)
            printf("STILL BLACK — TRANSFER_FROM_HOST did not bring rendered content!\n");
        else
            printf("GOT PIXELS! VirGL rendering confirmed working on host.\n");

        /* Print a few more samples for diagnosis */
        printf("pixel[0,1] = 0x%08X\n", pixels[stride/4]);
        printf("pixel[last] = 0x%08X\n", pixels[(height-1) * (stride/4) + width - 1]);
    }

    /* === TRANSFER_TO_HOST_3D: sync guest backing to host for display === */
    printf("\n=== TRANSFER_TO_HOST_3D (guest→host, for display pipeline) ===\n");
    {
        struct drm_virtgpu_3d_transfer_to_host xfer;
        memset(&xfer, 0, sizeof(xfer));
        xfer.bo_handle = bo_handle;
        xfer.stride = stride;
        xfer.box.w = width;
        xfer.box.h = height;
        xfer.box.d = 1;
        int tr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST, &xfer);
        if (tr < 0)
            printf("TRANSFER_TO_HOST: FAILED — %s\n", strerror(errno));
        else
            printf("TRANSFER_TO_HOST: OK\n");
    }

    /* Wait after TO_HOST */
    {
        struct drm_virtgpu_3d_wait wait;
        memset(&wait, 0, sizeof(wait));
        wait.handle = bo_handle;
        int wr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
        printf("WAIT after TO_HOST: %s\n", wr < 0 ? strerror(errno) : "OK");
    }

    /* === Also try CPU-fill as fallback verification === */
    if (pixels) {
        printf("\n=== CPU-fill fallback: writing GREEN directly to guest backing ===\n");
        /* B8G8R8X8_UNORM: byte order is B,G,R,X in memory
         * For GREEN: B=0x00, G=0xFF, R=0x00, X=0xFF → little-endian u32 = 0xFF00FF00 */
        uint32_t green_pixel = 0xFF00FF00;
        printf("Writing 0x%08X to all %u pixels...\n", green_pixel, width * height);
        for (uint32_t row = 0; row < height; row++) {
            uint32_t *rowptr = pixels + row * (stride / 4);
            for (uint32_t col = 0; col < width; col++)
                rowptr[col] = green_pixel;
        }
        printf("CPU fill complete\n");

        /* Transfer CPU-written pixels to host */
        struct drm_virtgpu_3d_transfer_to_host xfer;
        memset(&xfer, 0, sizeof(xfer));
        xfer.bo_handle = bo_handle;
        xfer.stride = stride;
        xfer.box.w = width;
        xfer.box.h = height;
        xfer.box.d = 1;
        int tr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST, &xfer);
        printf("TRANSFER_TO_HOST (after CPU fill): %s\n", tr < 0 ? strerror(errno) : "OK");

        struct drm_virtgpu_3d_wait wait;
        memset(&wait, 0, sizeof(wait));
        wait.handle = bo_handle;
        drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
    }

    /* === Step 3: Display via DRM KMS === */
    printf("\n=== DRM KMS Display ===\n");
    printf("Using stride=%u for AddFB\n", stride);

    uint32_t fb_id = 0;
    int ret = drmModeAddFB(drm_fd, width, height, 24, 32, stride, bo_handle, &fb_id);
    if (ret < 0) {
        fprintf(stderr, "drmModeAddFB failed: %s\n", strerror(errno));
        return 1;
    }
    printf("AddFB: fb_id=%u (depth=24, bpp=32, stride=%u, bo=%u)\n",
           fb_id, stride, bo_handle);

    ret = drmModeSetCrtc(drm_fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode);
    if (ret < 0) {
        fprintf(stderr, "drmModeSetCrtc failed: %s\n", strerror(errno));
        drmModeRmFB(drm_fd, fb_id);
        return 1;
    }
    printf("SetCrtc: OK — display should show GREEN\n\n");

    /* DirtyFB to trigger display update */
    if (drm_mark_dirty(fb_id, width, height) == 0)
        printf("DirtyFB: OK\n");
    else
        printf("DirtyFB: failed (non-fatal)\n");

    /* PageFlip for good measure */
    if (drm_page_flip_and_wait(fb_id) == 0)
        printf("PageFlip: OK\n");
    else
        printf("PageFlip: failed (non-fatal)\n");

    /* Hold for 15 seconds so user can see the result */
    printf("\nHolding display for 15 seconds...\n");
    sleep(15);

    if (pixels)
        munmap(pixels, map_size);

    /* Restore original CRTC */
    if (saved_crtc) {
        drmModeSetCrtc(drm_fd, saved_crtc->crtc_id, saved_crtc->buffer_id,
                       saved_crtc->x, saved_crtc->y, &conn_id, 1,
                       &saved_crtc->mode);
        drmModeFreeCrtc(saved_crtc);
    }
    drmModeRmFB(drm_fd, fb_id);
    close(drm_fd);

    printf("Done.\n");
    return 0;
}
