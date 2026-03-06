/*
 * egl_green_test.c — Minimal EGL clear-to-green test.
 * Uses the exact same EGL/GBM/DRM pipeline as gl_display.c
 * but only does a solid green clear. No bouncing balls.
 *
 * Build:  gcc -O2 -o egl_green_test egl_green_test.c -ldrm -lgbm -lEGL -lGLESv2
 * Run:    sudo ./egl_green_test
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/select.h>

#include <xf86drm.h>
#include <xf86drmMode.h>
#include <gbm.h>

#include <EGL/egl.h>
#include <GLES2/gl2.h>

static int drm_fd = -1;
static uint32_t conn_id, crtc_id;
static drmModeModeInfo mode;
static drmModeCrtcPtr saved_crtc;

static void page_flip_handler(int fd, unsigned int frame,
    unsigned int sec, unsigned int usec, void *data) {
    (void)fd; (void)frame; (void)sec; (void)usec;
    *(int *)data = 0;
}

static int find_drm(void) {
    const char *cards[] = {"/dev/dri/card0", "/dev/dri/card1", NULL};
    for (int i = 0; cards[i]; i++) {
        int fd = open(cards[i], O_RDWR | O_CLOEXEC);
        if (fd < 0) continue;
        if (drmSetMaster(fd) < 0) { close(fd); continue; }
        drmModeResPtr res = drmModeGetResources(fd);
        if (!res) { close(fd); continue; }
        drmModeConnectorPtr conn = NULL;
        for (int c = 0; c < res->count_connectors; c++) {
            conn = drmModeGetConnector(fd, res->connectors[c]);
            if (conn && conn->connection == DRM_MODE_CONNECTED && conn->count_modes > 0)
                break;
            if (conn) drmModeFreeConnector(conn);
            conn = NULL;
        }
        if (!conn) { drmModeFreeResources(res); close(fd); continue; }
        conn_id = conn->connector_id;
        mode = conn->modes[0];
        drmModeEncoderPtr enc = conn->encoder_id ? drmModeGetEncoder(fd, conn->encoder_id) : NULL;
        if (!enc && res->count_encoders > 0) enc = drmModeGetEncoder(fd, res->encoders[0]);
        crtc_id = enc ? enc->crtc_id : (res->count_crtcs > 0 ? res->crtcs[0] : 0);
        if (enc) drmModeFreeEncoder(enc);
        if (!crtc_id && res->count_crtcs > 0) crtc_id = res->crtcs[0];
        saved_crtc = drmModeGetCrtc(fd, crtc_id);
        printf("DRM: %s — %ux%u@%u, conn=%u, crtc=%u\n",
               cards[i], mode.hdisplay, mode.vdisplay, mode.vrefresh, conn_id, crtc_id);
        drmModeFreeConnector(conn);
        drmModeFreeResources(res);
        drm_fd = fd;
        return 0;
    }
    return -1;
}

int main(void) {
    printf("=== EGL Green Test — minimal clear-to-green ===\n\n");

    if (find_drm() < 0) { fprintf(stderr, "No DRM device\n"); return 1; }

    uint32_t W = mode.hdisplay, H = mode.vdisplay;

    /* GBM device + surface */
    struct gbm_device *gbm = gbm_create_device(drm_fd);
    if (!gbm) { fprintf(stderr, "gbm_create_device failed\n"); return 1; }

    struct gbm_surface *gbm_surf = gbm_surface_create(gbm, W, H,
        GBM_FORMAT_XRGB8888, GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING);
    if (!gbm_surf) { fprintf(stderr, "gbm_surface_create failed\n"); return 1; }
    printf("GBM surface created: %ux%u XRGB8888\n", W, H);

    /* EGL setup */
    EGLDisplay egl_dpy = eglGetDisplay((EGLNativeDisplayType)gbm);
    if (egl_dpy == EGL_NO_DISPLAY) { fprintf(stderr, "eglGetDisplay failed\n"); return 1; }
    EGLint major, minor;
    if (!eglInitialize(egl_dpy, &major, &minor)) { fprintf(stderr, "eglInitialize failed\n"); return 1; }
    printf("EGL %d.%d initialized\n", major, minor);

    if (!eglBindAPI(EGL_OPENGL_ES_API)) { fprintf(stderr, "eglBindAPI failed\n"); return 1; }

    EGLint cfg_attribs[] = {
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 0,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_NONE
    };
    EGLConfig cfg;
    EGLint num_cfg;
    eglChooseConfig(egl_dpy, cfg_attribs, &cfg, 1, &num_cfg);
    if (num_cfg == 0) { fprintf(stderr, "No EGL config\n"); return 1; }

    EGLint ctx_attribs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(egl_dpy, cfg, EGL_NO_CONTEXT, ctx_attribs);
    if (ctx == EGL_NO_CONTEXT) { fprintf(stderr, "eglCreateContext failed\n"); return 1; }

    EGLSurface egl_surf = eglCreateWindowSurface(egl_dpy, cfg, (EGLNativeWindowType)gbm_surf, NULL);
    if (egl_surf == EGL_NO_SURFACE) { fprintf(stderr, "eglCreateWindowSurface failed\n"); return 1; }

    eglMakeCurrent(egl_dpy, egl_surf, egl_surf, ctx);
    printf("GL_RENDERER: %s\n", glGetString(GL_RENDERER));
    printf("GL_VERSION: %s\n\n", glGetString(GL_VERSION));

    /* Render + display loop — 60 frames of solid green */
    uint32_t fb_id = 0;
    struct gbm_bo *prev_bo = NULL;
    uint32_t prev_fb = 0;
    int first = 1;

    for (int frame = 0; frame < 60; frame++) {
        /* Clear to bright green */
        glClearColor(0.0f, 1.0f, 0.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);

        eglSwapBuffers(egl_dpy, egl_surf);

        struct gbm_bo *bo = gbm_surface_lock_front_buffer(gbm_surf);
        uint32_t handle = gbm_bo_get_handle(bo).u32;
        uint32_t stride = gbm_bo_get_stride(bo);

        if (drmModeAddFB(drm_fd, W, H, 24, 32, stride, handle, &fb_id) < 0) {
            fprintf(stderr, "AddFB failed: %s\n", strerror(errno));
            break;
        }

        if (first) {
            /* First frame: SetCrtc to set mode */
            if (drmModeSetCrtc(drm_fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode) < 0) {
                fprintf(stderr, "SetCrtc failed: %s\n", strerror(errno));
                break;
            }
            printf("SetCrtc: OK (frame %d)\n", frame);
            first = 0;
        } else {
            /* Subsequent: PageFlip */
            int pending = 1;
            if (drmModePageFlip(drm_fd, crtc_id, fb_id, DRM_MODE_PAGE_FLIP_EVENT, &pending) < 0) {
                fprintf(stderr, "PageFlip failed: %s\n", strerror(errno));
                break;
            }
            drmEventContext ev;
            memset(&ev, 0, sizeof(ev));
            ev.version = DRM_EVENT_CONTEXT_VERSION;
            ev.page_flip_handler = page_flip_handler;
            while (pending) {
                fd_set fds;
                FD_ZERO(&fds);
                FD_SET(drm_fd, &fds);
                struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };
                select(drm_fd + 1, &fds, NULL, NULL, &tv);
                if (FD_ISSET(drm_fd, &fds))
                    drmHandleEvent(drm_fd, &ev);
            }
        }

        if (prev_bo) {
            drmModeRmFB(drm_fd, prev_fb);
            gbm_surface_release_buffer(gbm_surf, prev_bo);
        }
        prev_bo = bo;
        prev_fb = fb_id;

        if (frame % 15 == 0)
            printf("Frame %d OK\n", frame);
    }

    printf("\n=== Holding GREEN for 15 seconds ===\n");
    fflush(stdout);
    sleep(15);

    /* Cleanup */
    if (saved_crtc) {
        drmModeSetCrtc(drm_fd, saved_crtc->crtc_id, saved_crtc->buffer_id,
                       saved_crtc->x, saved_crtc->y, &conn_id, 1, &saved_crtc->mode);
        drmModeFreeCrtc(saved_crtc);
    }
    if (prev_bo) {
        drmModeRmFB(drm_fd, prev_fb);
        gbm_surface_release_buffer(gbm_surf, prev_bo);
    }
    eglDestroySurface(egl_dpy, egl_surf);
    eglDestroyContext(egl_dpy, ctx);
    eglTerminate(egl_dpy);
    gbm_surface_destroy(gbm_surf);
    gbm_device_destroy(gbm);
    close(drm_fd);
    printf("Done.\n");
    return 0;
}
