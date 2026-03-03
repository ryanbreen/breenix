/*
 * gl_display.c — VirGL rendering to the physical display via DRM/KMS + EGL
 *
 * This is the critical test: renders circles on the GPU via VirGL and
 * presents them to the physical display using DRM page flipping.
 * This is what Breenix needs to replicate.
 *
 * Build: gcc -O2 -o gl_display gl_display.c -lEGL -lGLESv2 -lgbm -ldrm -lm
 * Run:   ./gl_display [frames]  (default: 120)
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/mman.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <gbm.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

/* ------------------------------------------------------------------ */
/* Timing                                                              */
/* ------------------------------------------------------------------ */

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

/* ------------------------------------------------------------------ */
/* Shaders                                                             */
/* ------------------------------------------------------------------ */

static const char *vert_src =
    "attribute vec2 a_pos;\n"
    "attribute vec4 a_color;\n"
    "varying vec4 v_color;\n"
    "uniform vec2 u_resolution;\n"
    "void main() {\n"
    "    vec2 clip = (a_pos / u_resolution) * 2.0 - 1.0;\n"
    "    clip.y = -clip.y;\n"
    "    gl_Position = vec4(clip, 0.0, 1.0);\n"
    "    v_color = a_color;\n"
    "}\n";

static const char *frag_src =
    "precision mediump float;\n"
    "varying vec4 v_color;\n"
    "void main() {\n"
    "    gl_FragColor = v_color;\n"
    "}\n";

/* ------------------------------------------------------------------ */
/* Balls                                                               */
/* ------------------------------------------------------------------ */

#define NUM_BALLS 12
#define CIRCLE_SEGMENTS 20

struct Ball {
    float x, y, vx, vy, radius, r, g, b, mass;
};

static struct Ball balls[NUM_BALLS];
static int fb_width, fb_height;

static void init_balls(int w, int h) {
    fb_width = w;
    fb_height = h;
    struct Ball defaults[NUM_BALLS] = {
        { 100, 100,  11.0,  8.0, 38, 1.0, 0.2, 0.2, 38 },
        { 300, 200, -10.0,  7.0, 33, 0.2, 1.0, 0.2, 33 },
        { 200, 400,   9.0, -9.5, 42, 0.2, 0.2, 1.0, 42 },
        { 400, 300,  -8.5, -8.0, 28, 1.0, 1.0, 0.2, 28 },
        { 150, 300,  10.5,  6.0, 24, 1.0, 0.2, 1.0, 24 },
        { 350, 150,  -9.0,  7.5, 26, 0.2, 1.0, 1.0, 26 },
        { 450, 500,   8.0, -7.0, 35, 1.0, 0.6, 0.2, 35 },
        { 250, 550,  -7.5,  8.5, 30, 0.6, 0.2, 1.0, 30 },
        { 500, 100,   9.5,  9.5, 22, 0.8, 0.8, 0.8, 22 },
        { 120, 500, -11.0, -6.5, 20, 1.0, 0.4, 0.4, 20 },
        { 380, 450,   7.0,  9.0, 32, 0.4, 1.0, 0.4, 32 },
        { 520, 350,  -8.0, -8.5, 27, 0.4, 0.6, 1.0, 27 },
    };
    memcpy(balls, defaults, sizeof(defaults));
}

static void ball_bounce(struct Ball *b) {
    if (b->x - b->radius < 0)          { b->x = b->radius;              b->vx = -b->vx; }
    if (b->x + b->radius >= fb_width)   { b->x = fb_width - b->radius - 1;  b->vx = -b->vx; }
    if (b->y - b->radius < 0)          { b->y = b->radius;              b->vy = -b->vy; }
    if (b->y + b->radius >= fb_height)  { b->y = fb_height - b->radius - 1; b->vy = -b->vy; }
}

static void check_collision(struct Ball *a, struct Ball *b) {
    float dx = b->x - a->x, dy = b->y - a->y;
    float touch = a->radius + b->radius;
    float dist_sq = dx*dx + dy*dy;
    if (dist_sq >= touch*touch || dist_sq == 0) return;
    float dist = sqrtf(dist_sq);
    if (dist == 0) { a->x -= 1; b->x += 1; return; }
    float nx = dx/dist, ny = dy/dist;
    float v1n = a->vx*nx + a->vy*ny;
    float v2n = b->vx*nx + b->vy*ny;
    if (v1n <= v2n) return;
    float m1 = a->mass, m2 = b->mass, mt = m1+m2;
    float v1n_new = ((m1-m2)*v1n + 2*m2*v2n)/mt;
    float v2n_new = ((m2-m1)*v2n + 2*m1*v1n)/mt;
    a->vx += (v1n_new-v1n)*nx; a->vy += (v1n_new-v1n)*ny;
    b->vx += (v2n_new-v2n)*nx; b->vy += (v2n_new-v2n)*ny;
    float overlap = touch - dist + 0.5f;
    a->x -= overlap*m2/mt*nx; a->y -= overlap*m2/mt*ny;
    b->x += overlap*m1/mt*nx; b->y += overlap*m1/mt*ny;
}

static void physics_step(void) {
    for (int s = 0; s < 16; s++) {
        for (int i = 0; i < NUM_BALLS; i++) {
            balls[i].x += balls[i].vx/16.0f;
            balls[i].y += balls[i].vy/16.0f;
        }
        for (int i = 0; i < NUM_BALLS; i++) ball_bounce(&balls[i]);
        for (int i = 0; i < NUM_BALLS; i++)
            for (int j = i+1; j < NUM_BALLS; j++)
                check_collision(&balls[i], &balls[j]);
    }
}

/* ------------------------------------------------------------------ */
/* GL helpers                                                          */
/* ------------------------------------------------------------------ */

static GLuint compile_shader(GLenum type, const char *src) {
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &src, NULL);
    glCompileShader(s);
    GLint ok;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char buf[512];
        glGetShaderInfoLog(s, sizeof(buf), NULL, buf);
        fprintf(stderr, "Shader error: %s\n", buf);
        exit(1);
    }
    return s;
}

static int build_circle_verts(float *v, float cx, float cy, float r,
                               float cr, float cg, float cb) {
    int n = 0;
    v[n++] = cx; v[n++] = cy;
    v[n++] = cr; v[n++] = cg; v[n++] = cb; v[n++] = 1.0f;
    for (int i = 0; i <= CIRCLE_SEGMENTS; i++) {
        float angle = (float)i / CIRCLE_SEGMENTS * 2.0f * M_PI;
        v[n++] = cx + r * cosf(angle);
        v[n++] = cy + r * sinf(angle);
        v[n++] = cr; v[n++] = cg; v[n++] = cb; v[n++] = 1.0f;
    }
    return CIRCLE_SEGMENTS + 2;
}

/* ------------------------------------------------------------------ */
/* DRM + GBM + EGL display setup                                       */
/* ------------------------------------------------------------------ */

struct drm_state {
    int fd;
    drmModeConnector *connector;
    drmModeEncoder *encoder;
    drmModeCrtc *saved_crtc;
    uint32_t crtc_id;
    drmModeModeInfo mode;
};

static int find_drm_display(struct drm_state *drm) {
    const char *cards[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};
    for (int i = 0; cards[i]; i++) {
        drm->fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (drm->fd < 0) {
            fprintf(stderr, "  Cannot open %s: %s\n", cards[i], strerror(errno));
            continue;
        }

        /* Need master for modesetting */
        drmSetMaster(drm->fd);

        drmModeRes *res = drmModeGetResources(drm->fd);
        if (!res) {
            fprintf(stderr, "  %s: drmModeGetResources failed: %s\n", cards[i], strerror(errno));
            close(drm->fd);
            continue;
        }
        fprintf(stderr, "  %s: %d connectors, %d crtcs, %d encoders\n",
                cards[i], res->count_connectors, res->count_crtcs, res->count_encoders);

        /* Find connected connector */
        for (int c = 0; c < res->count_connectors; c++) {
            drm->connector = drmModeGetConnector(drm->fd, res->connectors[c]);
            if (!drm->connector) continue;
            if (drm->connector->connection == DRM_MODE_CONNECTED &&
                drm->connector->count_modes > 0) {
                printf("Found connector %d: %s, %dx%d\n",
                       drm->connector->connector_id,
                       drm->connector->count_modes > 0 ? "has modes" : "no modes",
                       drm->connector->modes[0].hdisplay,
                       drm->connector->modes[0].vdisplay);
                drm->mode = drm->connector->modes[0];
                break;
            }
            drmModeFreeConnector(drm->connector);
            drm->connector = NULL;
        }

        if (!drm->connector) {
            fprintf(stderr, "  %s: no connected connector found\n", cards[i]);
            drmModeFreeResources(res);
            close(drm->fd);
            continue;
        }

        /* Find encoder + CRTC */
        drm->encoder = drmModeGetEncoder(drm->fd, drm->connector->encoder_id);
        if (!drm->encoder) {
            /* Try first encoder */
            for (int e = 0; e < res->count_encoders; e++) {
                drm->encoder = drmModeGetEncoder(drm->fd, res->encoders[e]);
                if (drm->encoder) break;
            }
        }
        if (!drm->encoder) {
            fprintf(stderr, "No encoder found\n");
            drmModeFreeResources(res);
            continue;
        }

        drm->crtc_id = drm->encoder->crtc_id;
        if (!drm->crtc_id && res->count_crtcs > 0)
            drm->crtc_id = res->crtcs[0];

        drm->saved_crtc = drmModeGetCrtc(drm->fd, drm->crtc_id);

        printf("Using DRM device: %s\n", cards[i]);
        printf("Display: %dx%d @ %dHz\n",
               drm->mode.hdisplay, drm->mode.vdisplay, drm->mode.vrefresh);

        drmModeFreeResources(res);
        return 0;
    }
    return -1;
}

/* ------------------------------------------------------------------ */
/* Main                                                                */
/* ------------------------------------------------------------------ */

int main(int argc, char *argv[]) {
    int total_frames = 120;
    if (argc > 1) total_frames = atoi(argv[1]);
    if (total_frames <= 0) total_frames = 120;

    printf("=== GL Display: %d frames, DRM/KMS page-flip ===\n", total_frames);

    /* ---- Find DRM display ---- */
    struct drm_state drm = {0};
    if (find_drm_display(&drm) < 0) {
        fprintf(stderr, "No DRM display found\n");
        return 1;
    }

    int width = drm.mode.hdisplay;
    int height = drm.mode.vdisplay;
    init_balls(width, height);

    /* ---- GBM device ---- */
    struct gbm_device *gbm = gbm_create_device(drm.fd);
    if (!gbm) { fprintf(stderr, "GBM failed\n"); return 1; }

    /* ---- GBM surface ---- */
    struct gbm_surface *gbm_surf = gbm_surface_create(gbm, width, height,
                                                        GBM_FORMAT_XRGB8888,
                                                        GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING);
    if (!gbm_surf) { fprintf(stderr, "GBM surface failed\n"); return 1; }

    /* ---- EGL ---- */
    EGLDisplay dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_MESA, gbm, NULL);
    if (dpy == EGL_NO_DISPLAY)
        dpy = eglGetDisplay((EGLNativeDisplayType)gbm);
    if (dpy == EGL_NO_DISPLAY) { fprintf(stderr, "No EGL display\n"); return 1; }

    EGLint major, minor;
    eglInitialize(dpy, &major, &minor);
    eglBindAPI(EGL_OPENGL_ES_API);

    /* Enumerate ALL EGL configs and find one matching XRGB8888.
     * eglChooseConfig returns ARGB configs by default, which cause
     * drmModeSetCrtc EINVAL on Parallels — XRGB8888 is required. */
    EGLint total_configs = 0;
    eglGetConfigs(dpy, NULL, 0, &total_configs);
    EGLConfig *all_configs = malloc(total_configs * sizeof(EGLConfig));
    eglGetConfigs(dpy, all_configs, total_configs, &total_configs);
    printf("Scanning %d EGL configs for XRGB8888 match...\n", total_configs);

    EGLConfig config = NULL;
    for (int i = 0; i < total_configs; i++) {
        EGLint native_visual, render_type, surf_type, alpha_size;
        eglGetConfigAttrib(dpy, all_configs[i], EGL_NATIVE_VISUAL_ID, &native_visual);
        eglGetConfigAttrib(dpy, all_configs[i], EGL_RENDERABLE_TYPE, &render_type);
        eglGetConfigAttrib(dpy, all_configs[i], EGL_SURFACE_TYPE, &surf_type);
        eglGetConfigAttrib(dpy, all_configs[i], EGL_ALPHA_SIZE, &alpha_size);
        if (native_visual == (int)GBM_FORMAT_XRGB8888 &&
            (render_type & EGL_OPENGL_ES2_BIT) &&
            (surf_type & EGL_WINDOW_BIT)) {
            config = all_configs[i];
            printf("  Found XRGB8888 config #%d (alpha=%d)\n", i, alpha_size);
            break;
        }
    }
    free(all_configs);
    if (!config) { fprintf(stderr, "No XRGB8888 EGL config found\n"); return 1; }

    EGLint ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attrs);
    if (ctx == EGL_NO_CONTEXT) { fprintf(stderr, "eglCreateContext failed: 0x%x\n", eglGetError()); return 1; }

    EGLSurface egl_surf = eglCreateWindowSurface(dpy, config,
                                                   (EGLNativeWindowType)gbm_surf, NULL);
    if (egl_surf == EGL_NO_SURFACE) { fprintf(stderr, "eglCreateWindowSurface failed: 0x%x\n", eglGetError()); return 1; }

    eglMakeCurrent(dpy, egl_surf, egl_surf, ctx);

    printf("GL_RENDERER: %s\n", glGetString(GL_RENDERER));
    printf("GL_VERSION: %s\n", glGetString(GL_VERSION));

    /* ---- Compile shaders ---- */
    GLuint vs = compile_shader(GL_VERTEX_SHADER, vert_src);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, frag_src);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glBindAttribLocation(prog, 0, "a_pos");
    glBindAttribLocation(prog, 1, "a_color");
    glLinkProgram(prog);
    glUseProgram(prog);
    glUniform2f(glGetUniformLocation(prog, "u_resolution"), (float)width, (float)height);
    glViewport(0, 0, width, height);
    glDisable(GL_DEPTH_TEST);

    float *verts = malloc(NUM_BALLS * (CIRCLE_SEGMENTS + 2) * 6 * sizeof(float));
    GLuint vbo;
    glGenBuffers(1, &vbo);

    printf("Setup complete. Rendering %d frames to display...\n\n", total_frames);

    /* ---- Render loop with DRM page flipping ---- */
    uint64_t t_start = now_ns();
    uint64_t t_last_print = t_start;
    int frames_since_print = 0;
    struct gbm_bo *prev_bo = NULL;
    uint32_t prev_fb_id = 0;

    for (int frame = 0; frame < total_frames; frame++) {
        physics_step();

        /* Clear + draw */
        glClearColor(15.0f/255.0f, 15.0f/255.0f, 30.0f/255.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);

        for (int i = 0; i < NUM_BALLS; i++) {
            int nv = build_circle_verts(verts, balls[i].x, balls[i].y,
                                         balls[i].radius, balls[i].r, balls[i].g, balls[i].b);
            glBindBuffer(GL_ARRAY_BUFFER, vbo);
            glBufferData(GL_ARRAY_BUFFER, nv * 6 * sizeof(float), verts, GL_DYNAMIC_DRAW);
            glEnableVertexAttribArray(0);
            glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 6*sizeof(float), (void*)0);
            glEnableVertexAttribArray(1);
            glVertexAttribPointer(1, 4, GL_FLOAT, GL_FALSE, 6*sizeof(float), (void*)(2*sizeof(float)));
            glDrawArrays(GL_TRIANGLE_FAN, 0, nv);
        }

        /* eglSwapBuffers triggers the GPU render */
        eglSwapBuffers(dpy, egl_surf);

        /* Get the front buffer GBM BO */
        struct gbm_bo *bo = gbm_surface_lock_front_buffer(gbm_surf);
        if (!bo) {
            fprintf(stderr, "Failed to lock front buffer\n");
            continue;
        }

        uint32_t handle = gbm_bo_get_handle(bo).u32;
        uint32_t stride = gbm_bo_get_stride(bo);
        uint32_t fb_id = 0;

        /* Create DRM framebuffer from the GBM BO */
        int ret = drmModeAddFB(drm.fd, width, height, 24, 32, stride, handle, &fb_id);
        if (ret) {
            fprintf(stderr, "drmModeAddFB failed: %s\n", strerror(errno));
            gbm_surface_release_buffer(gbm_surf, bo);
            continue;
        }

        /* Set this buffer as the CRTC scanout (blocking page flip) */
        ret = drmModeSetCrtc(drm.fd, drm.crtc_id, fb_id, 0, 0,
                             &drm.connector->connector_id, 1, &drm.mode);
        if (ret) {
            fprintf(stderr, "drmModeSetCrtc failed: %s (frame %d)\n", strerror(errno), frame);
        }

        /* Release previous buffer */
        if (prev_bo) {
            drmModeRmFB(drm.fd, prev_fb_id);
            gbm_surface_release_buffer(gbm_surf, prev_bo);
        }
        prev_bo = bo;
        prev_fb_id = fb_id;

        frames_since_print++;
        if (frames_since_print >= 16) {
            uint64_t now = now_ns();
            uint64_t elapsed = now - t_last_print;
            if (elapsed > 0) {
                double fps = (double)frames_since_print * 1e9 / (double)elapsed;
                double ms = (double)elapsed / (double)frames_since_print / 1e6;
                printf("[frame %4d] FPS: %.1f  (%.2f ms/frame)\n", frame, fps, ms);
            }
            frames_since_print = 0;
            t_last_print = now;
        }
    }

    uint64_t t_end = now_ns();
    double total_secs = (double)(t_end - t_start) / 1e9;
    printf("\n=== Results ===\n");
    printf("Total frames: %d\n", total_frames);
    printf("Total time:   %.2f s\n", total_secs);
    printf("Average FPS:  %.1f\n", (double)total_frames / total_secs);

    /* Restore original display */
    if (drm.saved_crtc) {
        drmModeSetCrtc(drm.fd, drm.saved_crtc->crtc_id, drm.saved_crtc->buffer_id,
                       drm.saved_crtc->x, drm.saved_crtc->y,
                       &drm.connector->connector_id, 1, &drm.saved_crtc->mode);
        drmModeFreeCrtc(drm.saved_crtc);
    }

    /* Cleanup */
    if (prev_bo) {
        drmModeRmFB(drm.fd, prev_fb_id);
        gbm_surface_release_buffer(gbm_surf, prev_bo);
    }
    glDeleteBuffers(1, &vbo);
    free(verts);
    eglDestroySurface(dpy, egl_surf);
    gbm_surface_destroy(gbm_surf);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    close(drm.fd);

    return 0;
}
