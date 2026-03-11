/*
 * virgl_multi_texture_test.c — Multi-texture VirGL compositing test
 *
 * Proves that multiple VirGL TEXTURE_2D resources can be:
 *   1. Created independently
 *   2. Rendered to via separate SUBMIT_3D batches (CLEAR to different colors)
 *   3. Sampled from in a compositing pass that draws textured quads
 *
 * The final display shows:
 *   - Dark gray background
 *   - RED rectangle on the left  (texture A, pixels 100-500 x 100-400)
 *   - BLUE rectangle on the right (texture B, pixels 600-1000 x 100-400)
 *
 * Pixel readback verifies the composited result.
 *
 * Build:  gcc -O2 -o virgl_multi_texture_test virgl_multi_texture_test.c -ldrm
 * Run:    sudo ./virgl_multi_texture_test
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
#define VIRGL_CCMD_NOP                   0
#define VIRGL_CCMD_CREATE_OBJECT         1
#define VIRGL_CCMD_BIND_OBJECT           2
#define VIRGL_CCMD_SET_VIEWPORT_STATE    4
#define VIRGL_CCMD_SET_FRAMEBUFFER_STATE 5
#define VIRGL_CCMD_SET_VERTEX_BUFFERS    6
#define VIRGL_CCMD_CLEAR                 7
#define VIRGL_CCMD_DRAW_VBO             8
#define VIRGL_CCMD_RESOURCE_INLINE_WRITE 9
#define VIRGL_CCMD_SET_SAMPLER_VIEWS     10
#define VIRGL_CCMD_SET_SCISSOR_STATE     15
#define VIRGL_CCMD_SET_SUB_CTX           28
#define VIRGL_CCMD_CREATE_SUB_CTX        29
#define VIRGL_CCMD_BIND_SHADER           31
#define VIRGL_CCMD_SET_TWEAKS            46

/* Object types */
#define VIRGL_OBJ_BLEND           1
#define VIRGL_OBJ_RASTERIZER      2
#define VIRGL_OBJ_DSA             3
#define VIRGL_OBJ_SHADER          4
#define VIRGL_OBJ_VERTEX_ELEMENTS 5
#define VIRGL_OBJ_SAMPLER_VIEW    6
#define VIRGL_OBJ_SAMPLER_STATE   7
#define VIRGL_OBJ_SURFACE         8

/* Pipe constants */
#define PIPE_BUFFER        0
#define PIPE_TEXTURE_2D    2
#define PIPE_PRIM_TRIANGLE_STRIP 5

#define PIPE_FORMAT_B8G8R8X8_UNORM    2
#define PIPE_FORMAT_R32G32B32A32_FLOAT 31

#define PIPE_BIND_RENDER_TARGET   0x002
#define PIPE_BIND_SAMPLER_VIEW    0x008
#define PIPE_BIND_VERTEX_BUFFER   0x010
#define PIPE_BIND_SCANOUT         0x40000
#define PIPE_BIND_SHARED          0x100000

#define PIPE_CLEAR_COLOR0  0x04

#define PIPE_SHADER_VERTEX   0
#define PIPE_SHADER_FRAGMENT 1

#define PIPE_TEX_FILTER_LINEAR  1

/* =========================================================================
 * VirGL command buffer builder
 * ========================================================================= */

#define CMD_BUF_MAX 8192

static uint32_t cmd_buf[CMD_BUF_MAX];
static int cmd_len;

static void cmd_reset(void) { cmd_len = 0; }

static void cmd_push(uint32_t v)
{
    if (cmd_len < CMD_BUF_MAX)
        cmd_buf[cmd_len++] = v;
    else {
        fprintf(stderr, "FATAL: cmd_buf overflow at DWORD %d\n", cmd_len);
        exit(1);
    }
}

/* Build VirGL command header:
 *   bits [7:0]   = command opcode
 *   bits [15:8]  = object type (for create/bind commands)
 *   bits [31:16] = payload length in DWORDs (not including this header)
 */
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
 * VirGL command builders
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

static void cmd_set_tweaks(uint32_t id, uint32_t value)
{
    cmd_push(cmd0(VIRGL_CCMD_SET_TWEAKS, 0, 2));
    cmd_push(id);
    cmd_push(value);
}

/* Create shader with num_tokens=300 (Mesa default).
 * CRITICAL: num_tokens=0 silently corrupts the VirGL context. */
static void cmd_create_shader(uint32_t handle, uint32_t shader_type, const char *tgsi)
{
    int text_len = strlen(tgsi) + 1;
    int text_dwords = (text_len + 3) / 4;
    int payload_len = 5 + text_dwords;

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_SHADER, payload_len));
    cmd_push(handle);
    cmd_push(shader_type);
    cmd_push(text_len);     /* bit 31 clear = first/only chunk */
    cmd_push(300);           /* NUM_TOKENS = 300 (Mesa default, MUST be nonzero) */
    cmd_push(0);             /* num_so_outputs */
    push_tgsi_text(tgsi);
}

static void cmd_create_blend_simple(uint32_t handle)
{
    /* S0=0x04 (dither), S2[0]=0x78000000 (colormask=0xF<<27) — matches Mesa */
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_BLEND, 11));
    cmd_push(handle);
    cmd_push(0x00000004);   /* S0: dither enabled */
    cmd_push(0);            /* S1: logicop_func */
    cmd_push(0x78000000);   /* S2[0]: colormask=0xF<<27, blend disabled */
    cmd_push(0); cmd_push(0); cmd_push(0); /* S2[1..3] */
    cmd_push(0); cmd_push(0); cmd_push(0); /* S2[4..6] */
    cmd_push(0);                            /* S2[7] */
}

static void cmd_create_dsa_disabled(uint32_t handle)
{
    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_DSA, 5));
    cmd_push(handle);
    cmd_push(0);    /* S0: depth/alpha test disabled */
    cmd_push(0);    /* S1: front stencil disabled */
    cmd_push(0);    /* S2: back stencil disabled */
    cmd_push(0);    /* alpha_ref = 0.0f */
}

static void cmd_create_rasterizer_default(uint32_t handle)
{
    /* 0x60008082: depth_clip_near | point_quad | front_ccw | half_pixel | bottom_edge */
    uint32_t s0 = (1 << 1) | (1 << 7) | (1 << 15) | (1 << 29) | (1 << 30);

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_RASTERIZER, 9));
    cmd_push(handle);
    cmd_push(s0);                   /* 0x60008082 */
    cmd_push(f32_bits(1.0f));       /* point_size */
    cmd_push(0);                    /* sprite_coord_enable */
    cmd_push(0x0000FFFF);           /* clip_plane_enable = all */
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
    cmd_push(PIPE_CLEAR_COLOR0);         /* buffers = 0x04 */
    cmd_push(f32_bits(r));
    cmd_push(f32_bits(g));
    cmd_push(f32_bits(b));
    cmd_push(f32_bits(a));
    cmd_push(0x00000000);                /* depth f64 low */
    cmd_push(0x3FF00000);                /* depth f64 high = 1.0 */
    cmd_push(0);                         /* stencil */
}

/* Create sampler view for a TEXTURE_2D resource.
 * CRITICAL: bits [24:31] of the format DWORD must contain PIPE_TEXTURE_2D << 24.
 * Without this, the host creates a BUFFER-targeted sampler view and you get BLACK. */
static void cmd_create_sampler_view(uint32_t handle, uint32_t res_handle,
                                    uint32_t format, uint32_t first_level,
                                    uint32_t last_level, uint32_t swizzle_r,
                                    uint32_t swizzle_g, uint32_t swizzle_b,
                                    uint32_t swizzle_a)
{
    /* Format DWORD encoding:
     *   bits [5:0]   = PIPE_FORMAT
     *   bits [24:31] = texture target (PIPE_TEXTURE_2D = 2)
     * Swizzle DWORD encoding:
     *   bits [2:0]   = swizzle_r
     *   bits [5:3]   = swizzle_g
     *   bits [8:6]   = swizzle_b
     *   bits [11:9]  = swizzle_a
     */
    uint32_t format_dw = format | (PIPE_TEXTURE_2D << 24);
    uint32_t swizzle_dw = swizzle_r | (swizzle_g << 3) | (swizzle_b << 6) | (swizzle_a << 9);

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_SAMPLER_VIEW, 6));
    cmd_push(handle);
    cmd_push(res_handle);
    cmd_push(format_dw);
    cmd_push(first_level | (last_level << 8)); /* first_element / first_level + last_element / last_level */
    cmd_push(swizzle_dw);
    cmd_push(0);   /* buffer_offset (unused for TEXTURE_2D) */
}

/* Bind sampler views to a shader stage */
static void cmd_set_sampler_views(uint32_t shader_type, int count,
                                  uint32_t view_handles[])
{
    cmd_push(cmd0(VIRGL_CCMD_SET_SAMPLER_VIEWS, 0, count + 2));
    cmd_push(shader_type);
    cmd_push(0);  /* start_slot */
    for (int i = 0; i < count; i++)
        cmd_push(view_handles[i]);
}

/* Create sampler state (texture filtering) */
static void cmd_create_sampler_state(uint32_t handle,
                                     uint32_t wrap_s, uint32_t wrap_t, uint32_t wrap_r,
                                     uint32_t min_filter, uint32_t mag_filter,
                                     uint32_t mip_filter)
{
    /* S0 encoding (from virglrenderer):
     *   bits [2:0]   = wrap_s
     *   bits [5:3]   = wrap_t
     *   bits [8:6]   = wrap_r
     *   bits [11:9]  = min_img_filter
     *   bits [14:12] = min_mip_filter
     *   bits [17:15] = mag_img_filter
     *   bits [20:18] = compare_mode
     *   bits [23:21] = compare_func
     *   bit  24      = seamless_cube_map
     */
    uint32_t s0 = wrap_s | (wrap_t << 3) | (wrap_r << 6)
                | (min_filter << 9) | (mip_filter << 12) | (mag_filter << 15);

    cmd_push(cmd0(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJ_SAMPLER_STATE, 5));
    cmd_push(handle);
    cmd_push(s0);
    cmd_push(0);                    /* lod_bias (float) */
    cmd_push(0);                    /* min_lod (float) */
    cmd_push(f32_bits(1000.0f));    /* max_lod */
}

/* Bind sampler states */
static void cmd_bind_sampler_states(uint32_t shader_type, int count,
                                    uint32_t state_handles[])
{
    /* BIND_SAMPLER_STATES = VIRGL_CCMD_BIND_OBJECT with obj_type = SAMPLER_STATE
     * Actually it's a dedicated command: VIRGL_CCMD_BIND_SAMPLER_STATES = 3 */
    cmd_push(cmd0(3, 0, count + 2)); /* VIRGL_CCMD_BIND_SAMPLER_STATES = 3 */
    cmd_push(shader_type);
    cmd_push(0); /* start_slot */
    for (int i = 0; i < count; i++)
        cmd_push(state_handles[i]);
}

/* RESOURCE_INLINE_WRITE: write data directly into a VirGL resource.
 * Used for vertex buffer data. */
static void cmd_resource_inline_write(uint32_t res_handle, uint32_t level,
                                      uint32_t usage, uint32_t stride,
                                      uint32_t layer_stride,
                                      uint32_t x, uint32_t y, uint32_t z,
                                      uint32_t w, uint32_t h, uint32_t d,
                                      const void *data, uint32_t data_bytes)
{
    uint32_t data_dwords = (data_bytes + 3) / 4;
    cmd_push(cmd0(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + data_dwords));
    cmd_push(res_handle);
    cmd_push(level);
    cmd_push(usage);
    cmd_push(stride);
    cmd_push(layer_stride);
    cmd_push(x);
    cmd_push(y);
    cmd_push(z);
    cmd_push(w);
    cmd_push(h);
    cmd_push(d);
    /* Copy data as DWORDs */
    const uint8_t *bytes = (const uint8_t *)data;
    for (uint32_t i = 0; i < data_dwords; i++) {
        uint32_t dw = 0;
        for (int b = 0; b < 4; b++) {
            uint32_t idx = i * 4 + b;
            if (idx < data_bytes)
                dw |= ((uint32_t)bytes[idx]) << (b * 8);
        }
        cmd_push(dw);
    }
}

/* SET_VERTEX_BUFFERS: bind vertex buffers for drawing */
static void cmd_set_vertex_buffers(int count, uint32_t strides[],
                                   uint32_t offsets[], uint32_t res_handles[])
{
    cmd_push(cmd0(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, count * 3));
    for (int i = 0; i < count; i++) {
        cmd_push(strides[i]);
        cmd_push(offsets[i]);
        cmd_push(res_handles[i]);
    }
}

/* DRAW_VBO */
static void cmd_draw_vbo(uint32_t start, uint32_t count, uint32_t mode,
                          uint32_t indexed, uint32_t instance_count,
                          uint32_t min_index, uint32_t max_index)
{
    cmd_push(cmd0(VIRGL_CCMD_DRAW_VBO, 0, 12));
    cmd_push(start);
    cmd_push(count);
    cmd_push(mode);
    cmd_push(indexed);
    cmd_push(instance_count);
    cmd_push(0); /* index_bias */
    cmd_push(0); /* start_instance */
    cmd_push(0); /* primitive_restart */
    cmd_push(0); /* restart_index */
    cmd_push(min_index);
    cmd_push(max_index);
    cmd_push(0); /* cso (unused) */
}

/* =========================================================================
 * Hex dump
 * ========================================================================= */

static void hex_dump_dwords(const char *label, const uint32_t *data, int count)
{
    printf("[hex-dump] %s (%d DWORDs, %d bytes):\n", label, count, count * 4);
    for (int i = 0; i < count; i++) {
        printf("[hex-dump] %s +%03d (0x%03X): 0x%08X\n", label, i * 4, i * 4, data[i]);
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

        printf("DRM: %s -- %s %ux%u@%u\n", cards[i],
               conn->connector_type_id ? "connected" : "?",
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

static int virtgpu_wait(uint32_t bo_handle)
{
    struct drm_virtgpu_3d_wait wait;
    memset(&wait, 0, sizeof(wait));
    wait.handle = bo_handle;
    wait.flags = 0;
    return drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_WAIT, &wait);
}

static int virtgpu_transfer_from_host(uint32_t bo_handle, uint32_t stride,
                                       uint32_t width, uint32_t height)
{
    drm_virtgpu_3d_transfer_from_host xfer;
    memset(&xfer, 0, sizeof(xfer));
    xfer.bo_handle = bo_handle;
    xfer.stride = stride;
    xfer.box.w = width;
    xfer.box.h = height;
    xfer.box.d = 1;
    return drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_FROM_HOST, &xfer);
}

static int virtgpu_transfer_to_host(uint32_t bo_handle, uint32_t stride,
                                     uint32_t width, uint32_t height)
{
    struct drm_virtgpu_3d_transfer_to_host xfer;
    memset(&xfer, 0, sizeof(xfer));
    xfer.bo_handle = bo_handle;
    xfer.stride = stride;
    xfer.box.w = width;
    xfer.box.h = height;
    xfer.box.d = 1;
    return drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_TRANSFER_TO_HOST, &xfer);
}

/* =========================================================================
 * Texture dimensions and quad positions
 * ========================================================================= */

#define TEX_W 400
#define TEX_H 300

/* Quad A: pixels (100,100) to (500,400) — shows texture A (RED) */
#define QUAD_A_X0 100
#define QUAD_A_Y0 100
#define QUAD_A_X1 500
#define QUAD_A_Y1 400

/* Quad B: pixels (600,100) to (1000,400) — shows texture B (BLUE) */
#define QUAD_B_X0 600
#define QUAD_B_Y0 100
#define QUAD_B_X1 1000
#define QUAD_B_Y1 400

/* Pixel sample points for verification */
#define SAMPLE_RED_X   300   /* center of quad A */
#define SAMPLE_RED_Y   250
#define SAMPLE_BLUE_X  800   /* center of quad B */
#define SAMPLE_BLUE_Y  250
#define SAMPLE_GRAY_X  50    /* background area */
#define SAMPLE_GRAY_Y  50

/* =========================================================================
 * VirGL object handle allocation
 *
 * CRITICAL: VirGL object handles must be globally unique across ALL types.
 * virglrenderer uses a single hash table per sub-context.
 *
 * We use separate ranges to avoid collisions:
 *   Surfaces:         1-10
 *   Blend:            11
 *   DSA:              12
 *   Rasterizer:       13
 *   VS:               14
 *   FS (color):       15  (for clear batches — unused in composite)
 *   FS (texture):     16
 *   Vertex elements:  17
 *   Sampler view A:   18
 *   Sampler view B:   19
 *   Sampler state:    20
 *   VB resource:      created via DRM as resource 4
 * ========================================================================= */

#define HANDLE_SURFACE_A     1   /* surface for texture A (render-to) */
#define HANDLE_SURFACE_B     2   /* surface for texture B (render-to) */
#define HANDLE_SURFACE_DISP  3   /* surface for display resource (composite target) */
#define HANDLE_BLEND         11
#define HANDLE_DSA           12
#define HANDLE_RASTERIZER    13
#define HANDLE_VS            14
#define HANDLE_FS_TEXTURE    16
#define HANDLE_VE            17
#define HANDLE_SAMPLER_VIEW_A 18
#define HANDLE_SAMPLER_VIEW_B 19
#define HANDLE_SAMPLER_STATE  20

/* =========================================================================
 * Vertex data helpers
 * ========================================================================= */

/* Convert pixel coordinates to NDC (-1 to +1).
 * Note: Y is flipped (OpenGL convention: bottom = -1, top = +1).
 *   ndc_x = (pixel_x / screen_w) * 2.0 - 1.0
 *   ndc_y = 1.0 - (pixel_y / screen_h) * 2.0
 */
typedef struct {
    float pos[4];  /* x, y, z, w */
    float tex[4];  /* s, t, 0, 1 */
} vertex_t;

static void make_quad_vertices(vertex_t verts[4],
                               float px0, float py0, float px1, float py1,
                               float screen_w, float screen_h)
{
    float x0 = (px0 / screen_w) * 2.0f - 1.0f;
    float x1 = (px1 / screen_w) * 2.0f - 1.0f;
    float y0 = 1.0f - (py0 / screen_h) * 2.0f;  /* top (higher Y in pixels = lower in NDC) */
    float y1 = 1.0f - (py1 / screen_h) * 2.0f;  /* bottom */

    /* TRIANGLE_STRIP order: top-left, top-right, bottom-left, bottom-right */
    /* Vertex 0: top-left */
    verts[0] = (vertex_t){{ x0, y0, 0.0f, 1.0f }, { 0.0f, 0.0f, 0.0f, 1.0f }};
    /* Vertex 1: top-right */
    verts[1] = (vertex_t){{ x1, y0, 0.0f, 1.0f }, { 1.0f, 0.0f, 0.0f, 1.0f }};
    /* Vertex 2: bottom-left */
    verts[2] = (vertex_t){{ x0, y1, 0.0f, 1.0f }, { 0.0f, 1.0f, 0.0f, 1.0f }};
    /* Vertex 3: bottom-right */
    verts[3] = (vertex_t){{ x1, y1, 0.0f, 1.0f }, { 1.0f, 1.0f, 0.0f, 1.0f }};
}

/* =========================================================================
 * main
 * ========================================================================= */

int main(void)
{
    printf("=== VirGL Multi-Texture Compositing Test ===\n\n");

    /* Step 1: Find DRM device */
    if (find_drm_device() < 0)
        return 1;

    uint32_t width = mode.hdisplay;
    uint32_t height = mode.vdisplay;
    printf("Resolution: %ux%u\n\n", width, height);

    /* =====================================================================
     * Step 2: Create resources
     * ===================================================================== */

    /* Resource 1: Display surface (composited output) — 1920x1200, SCANOUT */
    struct drm_virtgpu_resource_create rc_disp;
    memset(&rc_disp, 0, sizeof(rc_disp));
    rc_disp.target = PIPE_TEXTURE_2D;
    rc_disp.format = PIPE_FORMAT_B8G8R8X8_UNORM;
    rc_disp.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW |
                   PIPE_BIND_SCANOUT | PIPE_BIND_SHARED;
    rc_disp.width = width;
    rc_disp.height = height;
    rc_disp.depth = 1;
    rc_disp.array_size = 1;

    printf("=== Creating display resource (res 1: %ux%u) ===\n", width, height);
    hex_dump_resource_create("RESOURCE_CREATE display", &rc_disp);
    if (virtgpu_resource_create(&rc_disp) < 0) return 1;
    printf("Display resource: bo=%u res=%u stride=%u size=%u\n\n",
           rc_disp.bo_handle, rc_disp.res_handle, rc_disp.stride, rc_disp.size);

    /* Resource 2: Texture A (RED window) — 400x300, no SCANOUT */
    struct drm_virtgpu_resource_create rc_texA;
    memset(&rc_texA, 0, sizeof(rc_texA));
    rc_texA.target = PIPE_TEXTURE_2D;
    rc_texA.format = PIPE_FORMAT_B8G8R8X8_UNORM;
    rc_texA.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW;
    rc_texA.width = TEX_W;
    rc_texA.height = TEX_H;
    rc_texA.depth = 1;
    rc_texA.array_size = 1;

    printf("=== Creating texture A (res 2: %ux%u) ===\n", TEX_W, TEX_H);
    hex_dump_resource_create("RESOURCE_CREATE texA", &rc_texA);
    if (virtgpu_resource_create(&rc_texA) < 0) return 1;
    printf("Texture A: bo=%u res=%u stride=%u size=%u\n\n",
           rc_texA.bo_handle, rc_texA.res_handle, rc_texA.stride, rc_texA.size);

    /* Resource 3: Texture B (BLUE window) — 400x300, no SCANOUT */
    struct drm_virtgpu_resource_create rc_texB;
    memset(&rc_texB, 0, sizeof(rc_texB));
    rc_texB.target = PIPE_TEXTURE_2D;
    rc_texB.format = PIPE_FORMAT_B8G8R8X8_UNORM;
    rc_texB.bind = PIPE_BIND_RENDER_TARGET | PIPE_BIND_SAMPLER_VIEW;
    rc_texB.width = TEX_W;
    rc_texB.height = TEX_H;
    rc_texB.depth = 1;
    rc_texB.array_size = 1;

    printf("=== Creating texture B (res 3: %ux%u) ===\n", TEX_W, TEX_H);
    hex_dump_resource_create("RESOURCE_CREATE texB", &rc_texB);
    if (virtgpu_resource_create(&rc_texB) < 0) return 1;
    printf("Texture B: bo=%u res=%u stride=%u size=%u\n\n",
           rc_texB.bo_handle, rc_texB.res_handle, rc_texB.stride, rc_texB.size);

    /* Resource 4: Vertex buffer (PIPE_BUFFER, VERTEX_BUFFER bind) */
    struct drm_virtgpu_resource_create rc_vb;
    memset(&rc_vb, 0, sizeof(rc_vb));
    rc_vb.target = PIPE_BUFFER;
    rc_vb.format = PIPE_FORMAT_R32G32B32A32_FLOAT;  /* doesn't matter for buffers, but Mesa uses this */
    rc_vb.bind = PIPE_BIND_VERTEX_BUFFER;
    rc_vb.width = 4096;  /* size in bytes (width for PIPE_BUFFER) */
    rc_vb.height = 1;
    rc_vb.depth = 1;
    rc_vb.array_size = 1;

    printf("=== Creating vertex buffer resource (res 4: buffer, 4096 bytes) ===\n");
    hex_dump_resource_create("RESOURCE_CREATE VB", &rc_vb);
    if (virtgpu_resource_create(&rc_vb) < 0) return 1;
    printf("VB resource: bo=%u res=%u\n\n",
           rc_vb.bo_handle, rc_vb.res_handle);

    /* Collect all BO handles for EXECBUFFER */
    uint32_t all_bos[4] = {
        rc_disp.bo_handle,
        rc_texA.bo_handle,
        rc_texB.bo_handle,
        rc_vb.bo_handle
    };

    /* =====================================================================
     * Step 3: Render to Texture A (RED)
     *
     * Each SUBMIT_3D batch must start with create_sub_ctx(1) + set_sub_ctx(1).
     * Objects do NOT survive create_sub_ctx — must recreate everything.
     * ===================================================================== */

    printf("=== Batch 1: Render RED to Texture A ===\n");
    cmd_reset();

    cmd_create_sub_ctx(1);
    cmd_set_sub_ctx(1);
    cmd_set_tweaks(1, 1);
    cmd_set_tweaks(2, TEX_W);

    /* Create surface for texture A's resource, set as framebuffer, clear RED */
    cmd_create_surface(HANDLE_SURFACE_A, rc_texA.res_handle,
                       PIPE_FORMAT_B8G8R8X8_UNORM, 0, 0);
    {
        uint32_t cbufs[] = { HANDLE_SURFACE_A };
        cmd_set_framebuffer_state(0, 1, cbufs);
    }
    cmd_clear_color(1.0f, 0.0f, 0.0f, 1.0f);  /* RED */

    hex_dump_dwords("BATCH_1_CLEAR_RED", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, all_bos, 4) < 0) return 1;
    virtgpu_wait(rc_texA.bo_handle);
    printf("Batch 1 (RED clear to texA): OK\n\n");

    /* =====================================================================
     * Step 4: Render to Texture B (BLUE)
     * ===================================================================== */

    printf("=== Batch 2: Render BLUE to Texture B ===\n");
    cmd_reset();

    cmd_create_sub_ctx(1);
    cmd_set_sub_ctx(1);
    cmd_set_tweaks(1, 1);
    cmd_set_tweaks(2, TEX_W);

    /* Create surface for texture B's resource, set as framebuffer, clear BLUE */
    cmd_create_surface(HANDLE_SURFACE_B, rc_texB.res_handle,
                       PIPE_FORMAT_B8G8R8X8_UNORM, 0, 0);
    {
        uint32_t cbufs[] = { HANDLE_SURFACE_B };
        cmd_set_framebuffer_state(0, 1, cbufs);
    }
    cmd_clear_color(0.0f, 0.0f, 1.0f, 1.0f);  /* BLUE */

    hex_dump_dwords("BATCH_2_CLEAR_BLUE", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, all_bos, 4) < 0) return 1;
    virtgpu_wait(rc_texB.bo_handle);
    printf("Batch 2 (BLUE clear to texB): OK\n\n");

    /* =====================================================================
     * Step 5: Composite both textures onto display resource
     *
     * This is the key batch that proves multi-texture sampling works:
     *   1. Clear display to dark gray
     *   2. Draw textured quad sampling from texture A at left position
     *   3. Switch sampler view to texture B, draw quad at right position
     * ===================================================================== */

    printf("=== Batch 3: Composite both textures onto display ===\n");
    cmd_reset();

    /* --- Sub-context setup --- */
    cmd_create_sub_ctx(1);
    cmd_set_sub_ctx(1);
    cmd_set_tweaks(1, 1);
    cmd_set_tweaks(2, width);

    /* --- Create display surface and set as framebuffer --- */
    cmd_create_surface(HANDLE_SURFACE_DISP, rc_disp.res_handle,
                       PIPE_FORMAT_B8G8R8X8_UNORM, 0, 0);
    {
        uint32_t cbufs[] = { HANDLE_SURFACE_DISP };
        cmd_set_framebuffer_state(0, 1, cbufs);
    }

    /* --- Clear display to dark gray background (0.2, 0.2, 0.2) --- */
    cmd_clear_color(0.2f, 0.2f, 0.2f, 1.0f);

    /* --- Create pipeline state objects --- */
    cmd_create_blend_simple(HANDLE_BLEND);
    cmd_bind_object(HANDLE_BLEND, VIRGL_OBJ_BLEND);

    cmd_create_dsa_disabled(HANDLE_DSA);
    cmd_bind_object(HANDLE_DSA, VIRGL_OBJ_DSA);

    cmd_create_rasterizer_default(HANDLE_RASTERIZER);
    cmd_bind_object(HANDLE_RASTERIZER, VIRGL_OBJ_RASTERIZER);

    /* --- Create and bind shaders --- */
    /* Vertex shader: passthrough position + texcoord */
    const char *vs_text =
        "VERT\n"
        "DCL IN[0]\n"
        "DCL IN[1]\n"
        "DCL OUT[0], POSITION\n"
        "DCL OUT[1], GENERIC[0]\n"
        "MOV OUT[0], IN[0]\n"
        "MOV OUT[1], IN[1]\n"
        "END\n";

    /* Fragment shader: sample texture and output */
    const char *fs_text =
        "FRAG\n"
        "PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n"
        "DCL IN[0], GENERIC[0], PERSPECTIVE\n"
        "DCL OUT[0], COLOR\n"
        "DCL SAMP[0]\n"
        "DCL SVIEW[0], 2D, FLOAT\n"
        "TEX OUT[0], IN[0], SAMP[0], 2D\n"
        "END\n";

    cmd_create_shader(HANDLE_VS, PIPE_SHADER_VERTEX, vs_text);
    cmd_bind_shader(HANDLE_VS, PIPE_SHADER_VERTEX);

    cmd_create_shader(HANDLE_FS_TEXTURE, PIPE_SHADER_FRAGMENT, fs_text);
    cmd_bind_shader(HANDLE_FS_TEXTURE, PIPE_SHADER_FRAGMENT);

    /* --- Create vertex elements (2 attributes: position + texcoord) ---
     * Each vertex has 8 floats: 4 for position, 4 for texcoord.
     * Attribute 0: offset=0, format=R32G32B32A32_FLOAT (position)
     * Attribute 1: offset=16, format=R32G32B32A32_FLOAT (texcoord)
     */
    {
        uint32_t offsets[] = { 0, 16 };
        uint32_t divisors[] = { 0, 0 };
        uint32_t vb_indices[] = { 0, 0 };
        uint32_t formats[] = { PIPE_FORMAT_R32G32B32A32_FLOAT,
                               PIPE_FORMAT_R32G32B32A32_FLOAT };
        cmd_create_vertex_elements(HANDLE_VE, 2, offsets, divisors, vb_indices, formats);
    }
    cmd_bind_object(HANDLE_VE, VIRGL_OBJ_VERTEX_ELEMENTS);

    /* --- Set viewport to full display --- */
    cmd_set_viewport((float)width, (float)height);

    /* --- Create sampler state (LINEAR filtering) --- */
    /* wrap modes: CLAMP_TO_EDGE = 2 */
    cmd_create_sampler_state(HANDLE_SAMPLER_STATE, 2, 2, 2,
                             PIPE_TEX_FILTER_LINEAR, PIPE_TEX_FILTER_LINEAR, 0);
    {
        uint32_t states[] = { HANDLE_SAMPLER_STATE };
        cmd_bind_sampler_states(PIPE_SHADER_FRAGMENT, 1, states);
    }

    /* --- Bind vertex buffer resource --- */
    {
        uint32_t strides[] = { sizeof(vertex_t) };  /* 32 bytes per vertex */
        uint32_t offsets[] = { 0 };
        uint32_t res_handles[] = { rc_vb.res_handle };
        cmd_set_vertex_buffers(1, strides, offsets, res_handles);
    }

    /* ---- Draw Quad A (texture A = RED) at left position ---- */

    /* Create sampler view for texture A.
     * Swizzle: identity (R=0, G=1, B=2, A=3) */
    cmd_create_sampler_view(HANDLE_SAMPLER_VIEW_A, rc_texA.res_handle,
                            PIPE_FORMAT_B8G8R8X8_UNORM,
                            0, 0,    /* first_level, last_level */
                            0, 1, 2, 3);  /* RGBA identity swizzle */
    {
        uint32_t views[] = { HANDLE_SAMPLER_VIEW_A };
        cmd_set_sampler_views(PIPE_SHADER_FRAGMENT, 1, views);
    }

    /* Upload vertex data for quad A via RESOURCE_INLINE_WRITE */
    {
        vertex_t verts[4];
        make_quad_vertices(verts,
                           (float)QUAD_A_X0, (float)QUAD_A_Y0,
                           (float)QUAD_A_X1, (float)QUAD_A_Y1,
                           (float)width, (float)height);

        printf("Quad A vertices (NDC):\n");
        for (int i = 0; i < 4; i++) {
            printf("  v%d: pos=(%.4f, %.4f, %.4f, %.4f) tex=(%.4f, %.4f, %.4f, %.4f)\n",
                   i, verts[i].pos[0], verts[i].pos[1], verts[i].pos[2], verts[i].pos[3],
                   verts[i].tex[0], verts[i].tex[1], verts[i].tex[2], verts[i].tex[3]);
        }

        /* Write quad A vertices at offset 0 in the VB resource */
        cmd_resource_inline_write(rc_vb.res_handle, 0, 0, 0, 0,
                                  0, 0, 0,                      /* x, y, z */
                                  sizeof(verts), 1, 1,          /* w, h, d (bytes for buffer) */
                                  verts, sizeof(verts));
    }

    /* Draw quad A: 4 vertices, TRIANGLE_STRIP */
    cmd_draw_vbo(0, 4, PIPE_PRIM_TRIANGLE_STRIP, 0, 1, 0, 3);

    /* ---- Draw Quad B (texture B = BLUE) at right position ---- */

    /* Create sampler view for texture B */
    cmd_create_sampler_view(HANDLE_SAMPLER_VIEW_B, rc_texB.res_handle,
                            PIPE_FORMAT_B8G8R8X8_UNORM,
                            0, 0,
                            0, 1, 2, 3);
    {
        uint32_t views[] = { HANDLE_SAMPLER_VIEW_B };
        cmd_set_sampler_views(PIPE_SHADER_FRAGMENT, 1, views);
    }

    /* Upload vertex data for quad B via RESOURCE_INLINE_WRITE */
    {
        vertex_t verts[4];
        make_quad_vertices(verts,
                           (float)QUAD_B_X0, (float)QUAD_B_Y0,
                           (float)QUAD_B_X1, (float)QUAD_B_Y1,
                           (float)width, (float)height);

        printf("Quad B vertices (NDC):\n");
        for (int i = 0; i < 4; i++) {
            printf("  v%d: pos=(%.4f, %.4f, %.4f, %.4f) tex=(%.4f, %.4f, %.4f, %.4f)\n",
                   i, verts[i].pos[0], verts[i].pos[1], verts[i].pos[2], verts[i].pos[3],
                   verts[i].tex[0], verts[i].tex[1], verts[i].tex[2], verts[i].tex[3]);
        }

        /* Write quad B vertices at offset 128 to avoid overwriting quad A
         * (4 vertices * 32 bytes = 128 bytes for quad A) */
        cmd_resource_inline_write(rc_vb.res_handle, 0, 0, 0, 0,
                                  128, 0, 0,                    /* x=128 (byte offset), y, z */
                                  sizeof(verts), 1, 1,          /* w, h, d */
                                  verts, sizeof(verts));
    }

    /* Re-bind vertex buffer with offset 128 for quad B */
    {
        uint32_t strides[] = { sizeof(vertex_t) };
        uint32_t offsets[] = { 128 };
        uint32_t res_handles[] = { rc_vb.res_handle };
        cmd_set_vertex_buffers(1, strides, offsets, res_handles);
    }

    /* Draw quad B: 4 vertices, TRIANGLE_STRIP */
    cmd_draw_vbo(0, 4, PIPE_PRIM_TRIANGLE_STRIP, 0, 1, 0, 3);

    hex_dump_dwords("BATCH_3_COMPOSITE", cmd_buf, cmd_len);

    if (virtgpu_execbuffer(cmd_buf, cmd_len, all_bos, 4) < 0) return 1;
    virtgpu_wait(rc_disp.bo_handle);
    printf("Batch 3 (composite both textures): OK\n\n");

    /* =====================================================================
     * Step 6: Display via DRM KMS
     * ===================================================================== */

    printf("=== Displaying composited result ===\n");

    /* TRANSFER_TO_HOST to sync for display */
    uint32_t disp_stride = rc_disp.stride;
    if (disp_stride == 0) disp_stride = width * 4;

    if (virtgpu_transfer_to_host(rc_disp.bo_handle, disp_stride, width, height) < 0)
        printf("TRANSFER_TO_HOST (display): failed\n");
    else
        printf("TRANSFER_TO_HOST (display): OK\n");
    virtgpu_wait(rc_disp.bo_handle);

    uint32_t fb_id = 0;
    int ret = drmModeAddFB(drm_fd, width, height, 24, 32,
                           disp_stride, rc_disp.bo_handle, &fb_id);
    if (ret < 0) {
        fprintf(stderr, "drmModeAddFB failed: %s\n", strerror(errno));
        return 1;
    }
    printf("AddFB: fb_id=%u\n", fb_id);

    ret = drmModeSetCrtc(drm_fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode);
    if (ret < 0) {
        fprintf(stderr, "drmModeSetCrtc failed: %s\n", strerror(errno));
        drmModeRmFB(drm_fd, fb_id);
        return 1;
    }
    printf("SetCrtc: OK -- display should show gray background + RED left + BLUE right\n\n");

    /* Mark dirty to trigger display update */
    {
        drmModeClip clip = { 0, 0, (uint16_t)width, (uint16_t)height };
        drmModeDirtyFB(drm_fd, fb_id, &clip, 1);
    }

    /* =====================================================================
     * Step 7: Readback + pixel verification
     * ===================================================================== */

    printf("=== Pixel readback verification ===\n");

    /* TRANSFER_FROM_HOST to get rendered pixels into guest backing */
    if (virtgpu_transfer_from_host(rc_disp.bo_handle, disp_stride, width, height) < 0) {
        printf("TRANSFER_FROM_HOST: FAILED\n");
    } else {
        printf("TRANSFER_FROM_HOST: OK\n");
    }
    virtgpu_wait(rc_disp.bo_handle);

    /* MAP the display resource */
    struct drm_virtgpu_map vmap;
    memset(&vmap, 0, sizeof(vmap));
    vmap.handle = rc_disp.bo_handle;
    uint32_t *pixels = NULL;
    uint32_t map_size = disp_stride * height;

    if (drmIoctl(drm_fd, DRM_IOCTL_VIRTGPU_MAP, &vmap) < 0) {
        printf("VIRTGPU_MAP: FAILED -- %s\n", strerror(errno));
    } else {
        pixels = mmap(NULL, map_size, PROT_READ | PROT_WRITE,
                      MAP_SHARED, drm_fd, vmap.offset);
        if (pixels == MAP_FAILED) {
            printf("mmap: FAILED -- %s\n", strerror(errno));
            pixels = NULL;
        } else {
            printf("mmap: OK (%u bytes at %p)\n", map_size, (void *)pixels);
        }
    }

    int pass_count = 0;
    int fail_count = 0;

    if (pixels) {
        uint32_t stride_px = disp_stride / 4;

        /* Sample pixel at center of quad A — should be RED.
         * B8G8R8X8_UNORM byte order: B, G, R, X in memory.
         * RED = B=0x00, G=0x00, R=0xFF, X=0xFF => LE u32 = 0xFF0000FF
         * Or X might be 0x00 => 0x000000FF
         * Actually in B8G8R8X8: byte[0]=B, byte[1]=G, byte[2]=R, byte[3]=X
         * As LE uint32: (X << 24) | (R << 16) | (G << 8) | B
         * RED: B=0, G=0, R=0xFF => 0x??FF0000 where ?? depends on X channel */
        uint32_t px_red = pixels[SAMPLE_RED_Y * stride_px + SAMPLE_RED_X];
        uint32_t px_blue = pixels[SAMPLE_BLUE_Y * stride_px + SAMPLE_BLUE_X];
        uint32_t px_gray = pixels[SAMPLE_GRAY_Y * stride_px + SAMPLE_GRAY_X];

        printf("\nPixel samples (B8G8R8X8_UNORM as LE uint32):\n");
        printf("  (%d,%d) = 0x%08X  (expect RED:  R channel high, B/G low)\n",
               SAMPLE_RED_X, SAMPLE_RED_Y, px_red);
        printf("  (%d,%d) = 0x%08X  (expect BLUE: B channel high, R/G low)\n",
               SAMPLE_BLUE_X, SAMPLE_BLUE_Y, px_blue);
        printf("  (%d,%d) = 0x%08X  (expect GRAY: R=G=B ~0x33)\n",
               SAMPLE_GRAY_X, SAMPLE_GRAY_Y, px_gray);

        /* Extract channels from B8G8R8X8_UNORM (LE):
         *   B = byte 0 = bits [7:0]
         *   G = byte 1 = bits [15:8]
         *   R = byte 2 = bits [23:16]
         *   X = byte 3 = bits [31:24]
         */
        #define GET_B(px) ((px) & 0xFF)
        #define GET_G(px) (((px) >> 8) & 0xFF)
        #define GET_R(px) (((px) >> 16) & 0xFF)

        /* Check RED pixel: R should be high (>= 0xC0), B and G should be low (<= 0x40) */
        uint8_t r_r = GET_R(px_red), r_g = GET_G(px_red), r_b = GET_B(px_red);
        printf("\n  RED check:  R=%u G=%u B=%u  ", r_r, r_g, r_b);
        if (r_r >= 0xC0 && r_g <= 0x40 && r_b <= 0x40) {
            printf("PASS\n");
            pass_count++;
        } else {
            printf("FAIL\n");
            fail_count++;
        }

        /* Check BLUE pixel: B should be high, R and G should be low */
        uint8_t b_r = GET_R(px_blue), b_g = GET_G(px_blue), b_b = GET_B(px_blue);
        printf("  BLUE check: R=%u G=%u B=%u  ", b_r, b_g, b_b);
        if (b_b >= 0xC0 && b_r <= 0x40 && b_g <= 0x40) {
            printf("PASS\n");
            pass_count++;
        } else {
            printf("FAIL\n");
            fail_count++;
        }

        /* Check GRAY pixel: R, G, B should all be similar and in ~0x20-0x40 range
         * 0.2 * 255 = 51 = 0x33 */
        uint8_t g_r = GET_R(px_gray), g_g = GET_G(px_gray), g_b = GET_B(px_gray);
        printf("  GRAY check: R=%u G=%u B=%u  ", g_r, g_g, g_b);
        if (g_r >= 0x20 && g_r <= 0x50 &&
            g_g >= 0x20 && g_g <= 0x50 &&
            g_b >= 0x20 && g_b <= 0x50 &&
            abs((int)g_r - (int)g_g) < 0x10 &&
            abs((int)g_r - (int)g_b) < 0x10) {
            printf("PASS\n");
            pass_count++;
        } else {
            printf("FAIL\n");
            fail_count++;
        }

        /* Print additional diagnostic pixels */
        printf("\nAdditional pixel samples:\n");
        /* Top-left of quad A */
        printf("  (%d,%d) = 0x%08X  (quad A top-left)\n",
               QUAD_A_X0 + 5, QUAD_A_Y0 + 5,
               pixels[(QUAD_A_Y0 + 5) * stride_px + QUAD_A_X0 + 5]);
        /* Top-left of quad B */
        printf("  (%d,%d) = 0x%08X  (quad B top-left)\n",
               QUAD_B_X0 + 5, QUAD_B_Y0 + 5,
               pixels[(QUAD_B_Y0 + 5) * stride_px + QUAD_B_X0 + 5]);
        /* Between the quads (should be gray) */
        printf("  (550,250) = 0x%08X  (between quads, expect gray)\n",
               pixels[250 * stride_px + 550]);
        /* Bottom-right corner (should be gray) */
        printf("  (%u,%u) = 0x%08X  (bottom-right corner)\n",
               width - 5, height - 5,
               pixels[(height - 5) * stride_px + width - 5]);

        munmap(pixels, map_size);
    } else {
        printf("Cannot verify pixels -- MAP failed\n");
        fail_count = 3;
    }

    /* =====================================================================
     * Final verdict
     * ===================================================================== */

    printf("\n========================================\n");
    if (fail_count == 0 && pass_count == 3) {
        printf("MULTI-TEXTURE TEST: PASS (%d/3 checks passed)\n", pass_count);
    } else {
        printf("MULTI-TEXTURE TEST: FAIL (%d passed, %d failed)\n", pass_count, fail_count);
    }
    printf("========================================\n\n");

    /* Hold display for 5 seconds */
    printf("Holding display for 5 seconds...\n");
    sleep(5);

    /* Cleanup */
    if (saved_crtc) {
        drmModeSetCrtc(drm_fd, saved_crtc->crtc_id, saved_crtc->buffer_id,
                       saved_crtc->x, saved_crtc->y, &conn_id, 1,
                       &saved_crtc->mode);
        drmModeFreeCrtc(saved_crtc);
    }
    drmModeRmFB(drm_fd, fb_id);
    close(drm_fd);

    printf("Done.\n");
    return (fail_count == 0) ? 0 : 1;
}
