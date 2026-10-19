import ctypes as C, os, struct, sys
os.environ['ANGLE_DEFAULT_PLATFORM'] = 'metal'
fw = os.environ['QEMU_GL_FRAMEWORKS'] + '/'
egl = C.CDLL(fw + 'EGL.framework/Versions/A/EGL')
def func(name, restype, args):
    f = getattr(egl, name); f.restype = restype; f.argtypes = args; return f
ptr = C.c_void_p; intp = C.POINTER(C.c_int)
getdisplay = func('eglGetDisplay', ptr, [ptr]); init = func('eglInitialize', C.c_uint, [ptr, intp, intp])
choose = func('eglChooseConfig', C.c_uint, [ptr, intp, C.POINTER(ptr), C.c_int, intp])
pbuffer = func('eglCreatePbufferSurface', ptr, [ptr, ptr, intp]); create = func('eglCreateContext', ptr, [ptr, ptr, ptr, intp])
current = func('eglMakeCurrent', C.c_uint, [ptr, ptr, ptr, ptr]); destroy = func('eglDestroyContext', C.c_uint, [ptr, ptr])
display = getdisplay(None); major = C.c_int(); minor = C.c_int(); assert init(display, C.byref(major), C.byref(minor))
attrs = (C.c_int * 7)(0x3033, 1, 0x3040, 0x0040, 0x3024, 8, 0x3038); cfg = ptr(); n = C.c_int()
assert choose(display, attrs, C.byref(cfg), 1, C.byref(n)) and n.value
surf = pbuffer(display, cfg, (C.c_int * 5)(0x3057, 1, 0x3056, 1, 0x3038)); assert surf
class Param(C.Structure):
    _fields_ = [('version', C.c_int), ('shared', C.c_bool), ('major', C.c_int), ('minor', C.c_int), ('compat', C.c_int)]
Create = C.CFUNCTYPE(ptr, ptr, C.c_int, C.POINTER(Param)); Destroy = C.CFUNCTYPE(None, ptr, ptr)
Current = C.CFUNCTYPE(C.c_int, ptr, C.c_int, ptr); Fence = C.CFUNCTYPE(None, ptr, C.c_uint)
contexts = []
@Create
def make(cookie, scan, p):
    a = (C.c_int * 5)(0x3098, p.contents.major, 0x30FB, p.contents.minor, 0x3038)
    ctx = create(display, cfg, contexts[0] if contexts and p.contents.shared else None, a)
    if ctx: contexts.append(ctx)
    return ctx
@Destroy
def drop(cookie, ctx): destroy(display, ctx)
@Current
def use(cookie, scan, ctx): return 0 if current(display, surf, surf, ctx) else -1
@Fence
def fence(cookie, n): pass
class Callbacks(C.Structure):
    _fields_ = [('version', C.c_int), ('write_fence', Fence), ('create', Create), ('destroy', Destroy), ('current', Current), ('other', ptr * 4)]
cb = Callbacks(); cb.version = 1; cb.write_fence = fence; cb.create = make; cb.destroy = drop; cb.current = use
lib = C.CDLL(fw + 'libvirglrenderer.1.dylib')
lib.virgl_renderer_init.argtypes = [ptr, C.c_int, C.POINTER(Callbacks)]
assert lib.virgl_renderer_init(ptr(1), 16, C.byref(cb)) == 0
lib.virgl_renderer_context_create.argtypes = [C.c_uint, C.c_uint, C.c_char_p]
lib.virgl_renderer_submit_cmd.argtypes = [ptr, C.c_int, C.c_int]
class Box(C.Structure):
    _fields_ = [(k, C.c_uint32) for k in 'x y z w h d'.split()]
class Iov(C.Structure):
    _fields_ = [('base', ptr), ('len', C.c_size_t)]
class Args(C.Structure):
    _fields_ = [(k, C.c_uint32) for k in 'handle target format bind width height depth array_size last_level nr_samples flags'.split()]
lib.virgl_renderer_resource_create.argtypes = [C.POINTER(Args), ptr, C.c_uint32]
lib.virgl_renderer_resource_attach_iov.argtypes = [C.c_int, C.POINTER(Iov), C.c_int]
lib.virgl_renderer_ctx_attach_resource.argtypes = [C.c_int, C.c_int]
lib.virgl_renderer_transfer_read_iov.argtypes = [C.c_uint32, C.c_uint32, C.c_uint32, C.c_uint32, C.c_uint32, C.POINTER(Box), C.c_uint64, C.POINTER(Iov), C.c_int]
lib.virgl_renderer_transfer_write_iov.argtypes = [C.c_uint32, C.c_uint32, C.c_int, C.c_uint32, C.c_uint32, C.POINTER(Box), C.c_uint64, C.POINTER(Iov), C.c_uint]

BGRA, R8, RGBA = 1, 64, 67
def cmd0(c, o, n): return c | o << 8 | n << 16
class Ctx:
    def __init__(self, cid):
        self.cid = cid; assert lib.virgl_renderer_context_create(cid, 4, b'g3x') == 0; self.words = []; self.keep = []
    def cmd(self, c, o, payload): self.words += [cmd0(c, o, len(payload))] + list(payload)
    def flush(self):
        data = struct.pack('<%dI' % len(self.words), *self.words); self.words = []
        buf = C.create_string_buffer(data); return lib.virgl_renderer_submit_cmd(buf, self.cid, len(data) // 4)
    def resource(self, h, target, fmt, bind, w, ht, backing):
        a = Args(h, target, fmt, bind, w, ht, 1, 1, 0, 0, 0); assert lib.virgl_renderer_resource_create(C.byref(a), None, 0) == 0
        if backing:
            mem = C.create_string_buffer(backing); self.keep.append(mem)
            iov = Iov(C.cast(mem, ptr), backing); assert lib.virgl_renderer_resource_attach_iov(h, C.byref(iov), 1) == 0
        lib.virgl_renderer_ctx_attach_resource(self.cid, h)
        return self.keep[-1] if backing else None
def shader_words(handle, kind, text, tokens):
    t = text.encode() + b'\0'; t += b'\0' * (-len(t) % 4)
    return [handle, kind, len(text) + 1, tokens, 0] + list(struct.unpack('<%dI' % (len(t) // 4), t))

def grammar(label, text, tokens, kind=1):
    c = Ctx(100 + grammar.n); grammar.n += 1
    c.cmd(1, 4, shader_words(1, kind, text, tokens)); r = c.flush()
    print(label, 'ACCEPT' if r == 0 else 'REFUSE', r, flush=True)
    lib.virgl_renderer_context_destroy(c.cid)
    return r
grammar.n = 0

if __name__ == '__main__':
    from pathlib import Path
    experiment = Path(sys.argv[1])
    exec(compile(experiment.read_text(), str(experiment), 'exec'), dict(globals(), __file__=str(experiment)))
    lib.virgl_renderer_cleanup(ptr(1))
