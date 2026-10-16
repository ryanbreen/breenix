// The planted negative control's OpenGL (scripts/qemu-gl/plant-software-renderer.sh). ANGLE's
// OpenGL backend opens OpenGL.framework by path and looks up CGLChoosePixelFormat in it; the planted
// ANGLE opens this library instead. It re-exports OpenGL.framework and adds one attribute to every
// pixel format request, kCGLPFARendererID = kCGLRendererGenericFloatID, so every context ANGLE
// creates runs on Apple's software renderer.
#include <OpenGL/OpenGL.h>
#include <dlfcn.h>

__attribute__((visibility("default")))
CGLError CGLChoosePixelFormat(const CGLPixelFormatAttribute *attribs, CGLPixelFormatObj *pix, GLint *npix)
{
    static CGLError (*apple)(const CGLPixelFormatAttribute *, CGLPixelFormatObj *, GLint *);
    CGLPixelFormatAttribute copy[64];
    int i = 0;

    if (!apple) {
        apple = (CGLError (*)(const CGLPixelFormatAttribute *, CGLPixelFormatObj *, GLint *))
            dlsym(RTLD_NEXT, "CGLChoosePixelFormat");
    }
    copy[i++] = kCGLPFARendererID;
    copy[i++] = (CGLPixelFormatAttribute)kCGLRendererGenericFloatID;
    for (int j = 0; attribs && attribs[j] && i < 62; j++) {
        copy[i++] = attribs[j];
    }
    copy[i] = (CGLPixelFormatAttribute)0;
    return apple(copy, pix, npix);
}
