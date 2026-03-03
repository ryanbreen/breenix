/*
 * gl_bench.c — Minimal OpenGL ES 2.0 benchmark using GBM + EGL + GLES2
 *
 * Renders filled circles (similar to the Breenix bounce demo) on the GPU
 * using VirGL, measures FPS, and optionally reads back pixels to verify
 * rendering actually works.
 *
 * This runs headless (no window system) using GBM + EGL, which is exactly
 * how a kernel's VirGL implementation would work.
 *
 * Build: gcc -O2 -o gl_bench gl_bench.c -lEGL -lGLESv2 -lgbm -lm
 * Run:   ./gl_bench [frames]  (default: 300 frames)
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <gbm.h>

/* ------------------------------------------------------------------ */
/* Timing helpers                                                      */
/* ------------------------------------------------------------------ */

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

/* ------------------------------------------------------------------ */
/* Shader source                                                       */
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
/* Ball definition                                                     */
/* ------------------------------------------------------------------ */

#define NUM_BALLS 12
#define CIRCLE_SEGMENTS 20
#define WIDTH  864
#define HEIGHT 1080

struct Ball {
    float x, y;        /* position in pixels */
    float vx, vy;      /* velocity in pixels/frame */
    float radius;
    float r, g, b;
    float mass;
};

static struct Ball balls[NUM_BALLS] = {
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

/* ------------------------------------------------------------------ */
/* Physics (identical to Breenix bounce demo)                          */
/* ------------------------------------------------------------------ */

static void ball_step(struct Ball *b) {
    b->x += b->vx;
    b->y += b->vy;
}

static void ball_bounce(struct Ball *b) {
    if (b->x - b->radius < 0)     { b->x = b->radius;              b->vx = -b->vx; }
    if (b->x + b->radius >= WIDTH) { b->x = WIDTH - b->radius - 1;  b->vx = -b->vx; }
    if (b->y - b->radius < 0)     { b->y = b->radius;              b->vy = -b->vy; }
    if (b->y + b->radius >= HEIGHT){ b->y = HEIGHT - b->radius - 1; b->vy = -b->vy; }
}

static void check_collision(struct Ball *a, struct Ball *b) {
    float dx = b->x - a->x;
    float dy = b->y - a->y;
    float touch = a->radius + b->radius;
    float dist_sq = dx*dx + dy*dy;
    if (dist_sq >= touch*touch || dist_sq == 0) return;

    float dist = sqrtf(dist_sq);
    if (dist == 0) { a->x -= 1; b->x += 1; return; }

    float nx = dx / dist;
    float ny = dy / dist;
    float v1n = a->vx*nx + a->vy*ny;
    float v2n = b->vx*nx + b->vy*ny;
    if (v1n <= v2n) return;

    float m1 = a->mass, m2 = b->mass, mt = m1 + m2;
    float v1n_new = ((m1-m2)*v1n + 2*m2*v2n) / mt;
    float v2n_new = ((m2-m1)*v2n + 2*m1*v1n) / mt;
    float dv1 = v1n_new - v1n;
    float dv2 = v2n_new - v2n;
    a->vx += dv1*nx; a->vy += dv1*ny;
    b->vx += dv2*nx; b->vy += dv2*ny;

    float overlap = touch - dist + 0.5f;
    float push1 = overlap * m2 / mt;
    float push2 = overlap * m1 / mt;
    a->x -= push1*nx; a->y -= push1*ny;
    b->x += push2*nx; b->y += push2*ny;
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
        fprintf(stderr, "Shader compile error: %s\n", buf);
        exit(1);
    }
    return s;
}

/* Build a triangle fan for a circle: center + N+1 edge vertices */
static int build_circle_vertices(float *verts, float cx, float cy, float r,
                                  float cr, float cg, float cb) {
    int n = 0;
    /* Center vertex: x, y, r, g, b, a */
    verts[n++] = cx; verts[n++] = cy;
    verts[n++] = cr; verts[n++] = cg; verts[n++] = cb; verts[n++] = 1.0f;

    for (int i = 0; i <= CIRCLE_SEGMENTS; i++) {
        float angle = (float)i / CIRCLE_SEGMENTS * 2.0f * M_PI;
        verts[n++] = cx + r * cosf(angle);
        verts[n++] = cy + r * sinf(angle);
        verts[n++] = cr; verts[n++] = cg; verts[n++] = cb; verts[n++] = 1.0f;
    }
    return (CIRCLE_SEGMENTS + 2); /* vertex count */
}

/* ------------------------------------------------------------------ */
/* Main                                                                */
/* ------------------------------------------------------------------ */

int main(int argc, char *argv[]) {
    int total_frames = 300;
    if (argc > 1) total_frames = atoi(argv[1]);
    if (total_frames <= 0) total_frames = 300;

    printf("=== GL Bench: %d frames, %dx%d, %d balls ===\n",
           total_frames, WIDTH, HEIGHT, NUM_BALLS);

    /* ---- Open DRM device ---- */
    int drm_fd = -1;
    const char *cards[] = {"/dev/dri/renderD128", "/dev/dri/card0", "/dev/dri/card1", NULL};
    for (int i = 0; cards[i]; i++) {
        drm_fd = open(cards[i], O_RDWR);
        if (drm_fd >= 0) {
            printf("Opened DRM device: %s\n", cards[i]);
            break;
        }
    }
    if (drm_fd < 0) {
        fprintf(stderr, "Failed to open any DRM device: %s\n", strerror(errno));
        return 1;
    }

    /* ---- GBM device ---- */
    struct gbm_device *gbm = gbm_create_device(drm_fd);
    if (!gbm) {
        fprintf(stderr, "Failed to create GBM device\n");
        return 1;
    }
    printf("GBM device created\n");

    /* ---- EGL setup ---- */
    /* Use eglGetPlatformDisplay (EGL 1.5) with GBM platform */
    EGLDisplay dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_MESA, gbm, NULL);
    if (dpy == EGL_NO_DISPLAY) {
        /* Fallback to legacy eglGetDisplay */
        dpy = eglGetDisplay((EGLNativeDisplayType)gbm);
    }
    if (dpy == EGL_NO_DISPLAY) {
        fprintf(stderr, "Failed to get EGL display\n");
        return 1;
    }

    EGLint major, minor;
    if (!eglInitialize(dpy, &major, &minor)) {
        fprintf(stderr, "eglInitialize failed: 0x%x\n", eglGetError());
        return 1;
    }
    printf("EGL %d.%d initialized\n", major, minor);

    const char *egl_vendor = eglQueryString(dpy, EGL_VENDOR);
    const char *egl_version = eglQueryString(dpy, EGL_VERSION);
    printf("EGL vendor: %s\n", egl_vendor ? egl_vendor : "unknown");
    printf("EGL version: %s\n", egl_version ? egl_version : "unknown");

    eglBindAPI(EGL_OPENGL_ES_API);

    EGLint cfg_attribs[] = {
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_NONE
    };
    EGLConfig config;
    EGLint num_configs;
    if (!eglChooseConfig(dpy, cfg_attribs, &config, 1, &num_configs) || num_configs == 0) {
        fprintf(stderr, "eglChooseConfig failed\n");
        return 1;
    }

    EGLint ctx_attribs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attribs);
    if (ctx == EGL_NO_CONTEXT) {
        fprintf(stderr, "eglCreateContext failed: 0x%x\n", eglGetError());
        return 1;
    }

    /* Create GBM surface for offscreen rendering */
    struct gbm_surface *gbm_surf = gbm_surface_create(gbm, WIDTH, HEIGHT,
                                                        GBM_FORMAT_ARGB8888,
                                                        GBM_BO_USE_RENDERING);
    if (!gbm_surf) {
        fprintf(stderr, "gbm_surface_create failed\n");
        return 1;
    }

    EGLSurface egl_surf = eglCreateWindowSurface(dpy, config,
                                                   (EGLNativeWindowType)gbm_surf, NULL);
    if (egl_surf == EGL_NO_SURFACE) {
        fprintf(stderr, "eglCreateWindowSurface failed: 0x%x\n", eglGetError());
        return 1;
    }

    if (!eglMakeCurrent(dpy, egl_surf, egl_surf, ctx)) {
        fprintf(stderr, "eglMakeCurrent failed: 0x%x\n", eglGetError());
        return 1;
    }

    printf("GL_RENDERER: %s\n", glGetString(GL_RENDERER));
    printf("GL_VENDOR: %s\n", glGetString(GL_VENDOR));
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
    GLint link_ok;
    glGetProgramiv(prog, GL_LINK_STATUS, &link_ok);
    if (!link_ok) {
        char buf[512];
        glGetProgramInfoLog(prog, sizeof(buf), NULL, buf);
        fprintf(stderr, "Link error: %s\n", buf);
        return 1;
    }
    glUseProgram(prog);
    GLint u_res = glGetUniformLocation(prog, "u_resolution");
    glUniform2f(u_res, (float)WIDTH, (float)HEIGHT);

    glViewport(0, 0, WIDTH, HEIGHT);
    glDisable(GL_DEPTH_TEST);

    printf("Shaders compiled, rendering %d frames...\n\n", total_frames);

    /* ---- Vertex buffer for circles ---- */
    /* Max vertices: NUM_BALLS * (CIRCLE_SEGMENTS + 2) * 6 floats */
    float *verts = malloc(NUM_BALLS * (CIRCLE_SEGMENTS + 2) * 6 * sizeof(float));
    if (!verts) { perror("malloc"); return 1; }

    GLuint vbo;
    glGenBuffers(1, &vbo);

    /* ---- Render loop ---- */
    uint64_t t_start = now_ns();
    uint64_t t_last_print = t_start;
    int frames_since_print = 0;

    for (int frame = 0; frame < total_frames; frame++) {
        /* Physics */
        for (int s = 0; s < 16; s++) {
            for (int i = 0; i < NUM_BALLS; i++) {
                balls[i].x += balls[i].vx / 16.0f;
                balls[i].y += balls[i].vy / 16.0f;
            }
            for (int i = 0; i < NUM_BALLS; i++) ball_bounce(&balls[i]);
            for (int i = 0; i < NUM_BALLS; i++)
                for (int j = i+1; j < NUM_BALLS; j++)
                    check_collision(&balls[i], &balls[j]);
        }

        /* Clear */
        glClearColor(15.0f/255.0f, 15.0f/255.0f, 30.0f/255.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);

        /* Draw each ball as a triangle fan */
        int total_verts_offset = 0;
        for (int i = 0; i < NUM_BALLS; i++) {
            int nv = build_circle_vertices(
                verts + total_verts_offset * 6,
                balls[i].x, balls[i].y, balls[i].radius,
                balls[i].r, balls[i].g, balls[i].b);

            glBindBuffer(GL_ARRAY_BUFFER, vbo);
            glBufferData(GL_ARRAY_BUFFER, nv * 6 * sizeof(float),
                         verts + total_verts_offset * 6, GL_DYNAMIC_DRAW);
            glEnableVertexAttribArray(0);
            glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 6*sizeof(float), (void*)0);
            glEnableVertexAttribArray(1);
            glVertexAttribPointer(1, 4, GL_FLOAT, GL_FALSE, 6*sizeof(float), (void*)(2*sizeof(float)));
            glDrawArrays(GL_TRIANGLE_FAN, 0, nv);
        }

        /* Swap / present */
        eglSwapBuffers(dpy, egl_surf);

        /* Release the GBM buffer so we can lock the next one */
        struct gbm_bo *bo = gbm_surface_lock_front_buffer(gbm_surf);
        if (bo) gbm_surface_release_buffer(gbm_surf, bo);

        frames_since_print++;

        /* Print FPS every 16 frames */
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

        /* Readback a few pixels on first frame to verify rendering */
        if (frame == 0) {
            unsigned char pixel[4];
            /* Read center of screen — should be background color */
            glReadPixels(WIDTH/2, HEIGHT/2, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
            printf("  Pixel at center: RGBA(%d, %d, %d, %d) — expect ~(15, 15, 30, 255)\n",
                   pixel[0], pixel[1], pixel[2], pixel[3]);

            /* Read where first ball should be */
            int bx = (int)balls[0].x;
            int by = HEIGHT - (int)balls[0].y; /* GL flips Y */
            if (bx >= 0 && bx < WIDTH && by >= 0 && by < HEIGHT) {
                glReadPixels(bx, by, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
                printf("  Pixel at ball[0] (%d,%d): RGBA(%d, %d, %d, %d) — expect red-ish\n",
                       bx, by, pixel[0], pixel[1], pixel[2], pixel[3]);
            }
        }
    }

    uint64_t t_end = now_ns();
    double total_secs = (double)(t_end - t_start) / 1e9;
    double avg_fps = (double)total_frames / total_secs;

    printf("\n=== Results ===\n");
    printf("Total frames: %d\n", total_frames);
    printf("Total time:   %.2f s\n", total_secs);
    printf("Average FPS:  %.1f\n", avg_fps);
    printf("Avg ms/frame: %.2f\n", total_secs * 1000.0 / total_frames);

    /* Cleanup */
    glDeleteBuffers(1, &vbo);
    glDeleteProgram(prog);
    free(verts);
    eglDestroySurface(dpy, egl_surf);
    gbm_surface_destroy(gbm_surf);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    close(drm_fd);

    return 0;
}
