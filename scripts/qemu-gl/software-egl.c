// The planted negative control's EGL (scripts/qemu-gl/plant-software-renderer.sh): it re-exports
// Mesa's libEGL, so the GL QEMU runs on Mesa's software renderer, llvmpipe, and only changes the
// surface type QEMU asks eglChooseConfig for. Mesa's surfaceless platform (EGL_PLATFORM=surfaceless,
// the only one it has on macOS) offers pbuffer configs, and QEMU asks for window configs, which a
// headless run never uses. Nothing else differs, so the run reaches the guest's first SUBMIT_3D and
// its attestation names llvmpipe.
#include <dlfcn.h>
#include <stdint.h>

typedef int32_t EGLint;
typedef unsigned int EGLBoolean;
typedef void *EGLDisplay;
typedef void *EGLConfig;

#define EGL_SURFACE_TYPE 0x3033
#define EGL_PBUFFER_BIT 0x0001
#define EGL_NONE 0x3038

__attribute__((visibility("default")))
EGLBoolean eglChooseConfig(EGLDisplay dpy, const EGLint *attribs, EGLConfig *configs, EGLint size, EGLint *count)
{
    static EGLBoolean (*mesa)(EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *);
    EGLint copy[64];
    int i = 0;

    if (!mesa) {
        mesa = (EGLBoolean (*)(EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *))
            dlsym(RTLD_NEXT, "eglChooseConfig");
    }
    for (; attribs && attribs[i] != EGL_NONE && i < 62; i += 2) {
        copy[i] = attribs[i];
        copy[i + 1] = attribs[i] == EGL_SURFACE_TYPE ? EGL_PBUFFER_BIT : attribs[i + 1];
    }
    copy[i] = EGL_NONE;
    return mesa(dpy, copy, configs, size, count);
}
