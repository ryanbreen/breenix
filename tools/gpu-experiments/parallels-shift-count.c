/*
 * parallels-shift-count.c: a black-box probe of how a host VirGL renderer treats a
 * variable shift count of 32 or more.
 *
 * The Gallium TGSI documentation defines SHL, USHR and ISHR on the low five bits of the
 * shift count (a count of 33 shifts by 1). This program runs on a Linux guest whose GPU
 * is the hypervisor's virtio-gpu with VirGL, through the guest's own GLES driver (used as
 * a library through EGL and GLES 3.0, ordinary use of its public API). A fragment
 * program shifts 0x80000001 left and right by a count read from a uniform array, so no
 * compiler can fold it, and writes the results to an RGBA32UI target that glReadPixels
 * returns. Run with the driver's TGSI dump enabled to show the shift reaches the host as
 * a bare variable-count USHR/SHL.
 *
 * Build: gcc -O2 -o parallels-shift-count parallels-shift-count.c -lgbm -lEGL -lGLESv2
 * Run:   ./parallels-shift-count [/dev/dri/renderD128]
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <gbm.h>

static const uint32_t COUNTS[8] = {0, 1, 31, 32, 33, 63, 64, 95};

static const char *VS =
    "#version 300 es\n"
    "in vec2 p;\n"
    "void main() { gl_Position = vec4(p, 0.0, 1.0); }\n";

static const char *FS =
    "#version 300 es\n"
    "precision highp float;\n"
    "precision highp int;\n"
    "uniform uint counts[8];\n"
    "uniform uint value;\n"
    "out uvec4 o;\n"
    "void main() {\n"
    "  uint s = counts[int(gl_FragCoord.x)];\n"
    "  o = uvec4(value >> s, value << s, value >> (s & 31u), value << (s & 31u));\n"
    "}\n";

static GLuint shader(GLenum kind, const char *text) {
    GLuint s = glCreateShader(kind);
    glShaderSource(s, 1, &text, NULL);
    glCompileShader(s);
    GLint ok = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[2048];
        glGetShaderInfoLog(s, sizeof log, NULL, log);
        fprintf(stderr, "shader: %s\n", log);
    }
    return s;
}

int main(int argc, char **argv) {
    const char *node = argc > 1 ? argv[1] : "/dev/dri/renderD128";
    int fd = open(node, O_RDWR | O_CLOEXEC);
    if (fd < 0) { perror(node); return 1; }
    struct gbm_device *gbm = gbm_create_device(fd);
    EGLDisplay dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, gbm, NULL);
    if (!eglInitialize(dpy, NULL, NULL)) { fprintf(stderr, "eglInitialize failed\n"); return 1; }
    eglBindAPI(EGL_OPENGL_ES_API);
    const EGLint ctx_attr[] = {EGL_CONTEXT_MAJOR_VERSION, 3, EGL_NONE};
    EGLContext ctx = eglCreateContext(dpy, EGL_NO_CONFIG_KHR, EGL_NO_CONTEXT, ctx_attr);
    if (ctx == EGL_NO_CONTEXT || !eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, ctx)) {
        fprintf(stderr, "no surfaceless GLES 3 context\n");
        return 1;
    }
    printf("GL_RENDERER %s\nGL_VERSION %s\n", glGetString(GL_RENDERER), glGetString(GL_VERSION));

    GLuint rb, fb;
    glGenRenderbuffers(1, &rb);
    glBindRenderbuffer(GL_RENDERBUFFER, rb);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA32UI, 8, 1);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rb);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        fprintf(stderr, "framebuffer incomplete\n");
        return 1;
    }

    GLuint prog = glCreateProgram();
    glAttachShader(prog, shader(GL_VERTEX_SHADER, VS));
    glAttachShader(prog, shader(GL_FRAGMENT_SHADER, FS));
    glBindAttribLocation(prog, 0, "p");
    glLinkProgram(prog);
    glUseProgram(prog);
    glUniform1uiv(glGetUniformLocation(prog, "counts"), 8, COUNTS);
    const uint32_t value = 0x80000001u;
    glUniform1ui(glGetUniformLocation(prog, "value"), value);

    const float tri[] = {-1, -1, 3, -1, -1, 3};
    GLuint vb;
    glGenBuffers(1, &vb);
    glBindBuffer(GL_ARRAY_BUFFER, vb);
    glBufferData(GL_ARRAY_BUFFER, sizeof tri, tri, GL_STATIC_DRAW);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, 0);
    glViewport(0, 0, 8, 1);
    glDrawArrays(GL_TRIANGLES, 0, 3);

    uint32_t px[8][4];
    memset(px, 0xA5, sizeof px);
    glReadPixels(0, 0, 8, 1, GL_RGBA_INTEGER, GL_UNSIGNED_INT, px);
    printf("value=0x%08x error=0x%x\n", value, glGetError());
    int masked = 1;
    for (int i = 0; i < 8; i++) {
        uint32_t s = COUNTS[i] & 31;
        uint32_t shr = value >> s, shl = value << s;
        int ok = px[i][0] == shr && px[i][1] == shl;
        masked &= ok && px[i][2] == shr && px[i][3] == shl;
        printf("count=%2u shr=0x%08x shl=0x%08x masked-shr=0x%08x masked-shl=0x%08x %s\n", COUNTS[i],
               px[i][0], px[i][1], px[i][2], px[i][3], ok ? "five-bit" : "NOT five-bit");
    }
    printf("SHIFT COUNT %s\n", masked ? "MASKED TO FIVE BITS" : "NOT MASKED TO FIVE BITS");
    return 0;
}
