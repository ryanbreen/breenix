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

#define DRM_VIRTGPU_EXECBUFFER       0x02
#define DRM_VIRTGPU_RESOURCE_CREATE  0x04
#define DRM_VIRTGPU_TRANSFER_TO_HOST 0x07
#define DRM_VIRTGPU_WAIT             0x08

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

struct drm_virtgpu_3d_wait {
    uint32_t handle;
    uint32_t flags;
};

#define DRM_IOCTL_VIRTGPU_EXECBUFFER \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_EXECBUFFER, \
             struct drm_virtgpu_execbuffer)

#define DRM_IOCTL_VIRTGPU_RESOURCE_CREATE \
    DRM_IOWR(DRM_COMMAND_BASE + DRM_VIRTGPU_RESOURCE_CREATE, \
             struct drm_virtgpu_resource_create)

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
    cmd_push(0);            /* S0: no special blend */
    cmd_push(0);            /* S1: logicop_func */
    cmd_push(0xF0000000);   /* S2[0]: colormask=0xF<<28, blend disabled */
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
    /* S0: depth_clip_near(bit1) | scissor(bit14) | half_pixel_center(bit29) */
    uint32_t s0 = (1 << 1) | (1 << 14) | (1 << 29);

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_RASTERIZER, 9));
    cmd_push(handle);
    cmd_push(s0);                   /* 0x20004002 */
    cmd_push(f32_bits(1.0f));       /* point_size */
    cmd_push(0);                    /* sprite_coord_enable */
    cmd_push(0);                    /* S3 */
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

    /* === Batch 1: Pipeline state creation === */
    printf("=== VirGL Batch 1: Pipeline State ===\n");
    cmd_reset();

    /* create_sub_ctx(1) + set_sub_ctx(1) */
    cmd_create_sub_ctx(1);
    cmd_set_sub_ctx(1);

    /* create_shader(handle=1, VERTEX, vs_text) */
    cmd_create_shader(1, PIPE_SHADER_VERTEX, vs_text);

    /* create_shader(handle=2, FRAGMENT, fs_text) */
    cmd_create_shader(2, PIPE_SHADER_FRAGMENT, fs_text);

    /* create_blend_simple(1) */
    cmd_create_blend_simple(1);

    /* create_dsa_disabled(1) */
    cmd_create_dsa_disabled(1);

    /* create_rasterizer_default(1) */
    cmd_create_rasterizer_default(1);

    /* create_vertex_elements(1, [(0,0,0,R32G32B32A32_FLOAT), (16,0,0,R32G32B32A32_FLOAT)]) */
    {
        uint32_t offsets[] = {0, 16};
        uint32_t divisors[] = {0, 0};
        uint32_t vb_indices[] = {0, 0};
        uint32_t formats[] = {PIPE_FORMAT_R32G32B32A32_FLOAT, PIPE_FORMAT_R32G32B32A32_FLOAT};
        cmd_create_vertex_elements(1, 2, offsets, divisors, vb_indices, formats);
    }

    hex_dump_dwords("BATCH_1_PIPELINE_STATE", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, &bo_handle, 1) < 0)
        return 1;
    printf("Batch 1 submitted OK\n\n");

    /* === Batch 2: Bind state + clear === */
    printf("=== VirGL Batch 2: Bind + Clear ===\n");
    cmd_reset();

    cmd_set_sub_ctx(1);

    /* Bind pipeline state */
    cmd_bind_shader(1, PIPE_SHADER_VERTEX);
    cmd_bind_shader(2, PIPE_SHADER_FRAGMENT);
    cmd_bind_object(1, VIRGL_OBJ_BLEND);
    cmd_bind_object(1, VIRGL_OBJ_DSA);
    cmd_bind_object(1, VIRGL_OBJ_RASTERIZER);
    cmd_bind_object(1, VIRGL_OBJ_VERTEX_ELEMENTS);

    cmd_set_viewport((float)width, (float)height);
    cmd_set_scissor_state(0, 0, width, height);

    /* create_surface(handle=1, res=res_handle, B8G8R8X8_UNORM, level=0, layers=0) */
    cmd_create_surface(1, res_handle, PIPE_FORMAT_B8G8R8X8_UNORM, 0, 0);

    /* set_framebuffer_state(zsurf=0, cbufs=[1]) */
    {
        uint32_t cbufs[] = {1};
        cmd_set_framebuffer_state(0, 1, cbufs);
    }

    /* clear_color: cornflower blue (0.392, 0.584, 0.929, 1.0) */
    cmd_clear_color(0.392f, 0.584f, 0.929f, 1.0f);

    hex_dump_dwords("BATCH_2_BIND_CLEAR", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, &bo_handle, 1) < 0)
        return 1;
    printf("Batch 2 submitted OK\n\n");

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

    /* === TRANSFER_TO_HOST_3D (sync resource to host) === */
    printf("=== TRANSFER_TO_HOST_3D ===\n");
    {
        struct drm_virtgpu_3d_transfer_to_host xfer;
        memset(&xfer, 0, sizeof(xfer));
        xfer.bo_handle = bo_handle;
        xfer.level = 0;
        xfer.stride = width * 4;
        xfer.layer_stride = 0;
        xfer.box.x = 0;
        xfer.box.y = 0;
        xfer.box.z = 0;
        xfer.box.w = width;
        xfer.box.h = height;
        xfer.box.d = 1;
        int tr = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST, &xfer);
        if (tr < 0)
            printf("TRANSFER_TO_HOST failed: %s (continuing anyway)\n", strerror(errno));
        else
            printf("TRANSFER_TO_HOST: OK\n");
    }

    /* Wait again after transfer */
    {
        struct drm_virtgpu_3d_wait wait2;
        memset(&wait2, 0, sizeof(wait2));
        wait2.handle = bo_handle;
        wait2.flags = 0;
        int wr2 = drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait2);
        if (wr2 < 0)
            printf("VIRTGPU_WAIT(2) failed: %s\n", strerror(errno));
        else
            printf("VIRTGPU_WAIT(2): OK — transfer complete\n");
    }
    printf("\n");

    /* === Step 3: Display via DRM KMS === */
    printf("=== DRM KMS Display ===\n");

    /* If stride is 0, compute it (width * 4) */
    if (stride == 0)
        stride = width * 4;
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
    printf("SetCrtc: OK — display should show CORNFLOWER BLUE\n\n");

    if (drm_page_flip_and_wait(fb_id) == 0) {
        printf("PageFlip: OK\n");
    } else {
        printf("PageFlip: failed, attempting DirtyFB\n");
        if (drm_mark_dirty(fb_id, width, height) == 0)
            printf("DirtyFB: OK\n");
    }

    /* Hold for 5 seconds so user can see the result */
    printf("Holding display for 5 seconds...\n");
    sleep(5);

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
