#!/usr/bin/env python3
"""Host-only VirGL/ANGLE shader-cache experiment; never launches QEMU.

Uses the production TGSI catalogue and VirGL LINK_SHADER to force compile/link.
Each mode runs in a fresh process, with two fresh VirGL contexts per process.
The disabled mode retains ANGLE's default display-memory cache; only the
candidate application's persistent blob callbacks are disabled.
"""
import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import time
import threading

ROOT = Path(__file__).resolve().parents[1]
P = C.c_void_p
I = C.c_int
U = C.c_uint


def catalogue():
    directory = ROOT / 'kernel/src/gpu/virgl'
    encoder = (directory / 'encode.rs').read_text()
    vertex = re.search(r'pub const SOLID_VS: Program.*?text: b"(.*?)",',
                       encoder, re.S).group(1).encode().decode('unicode_escape').encode()
    entries = [(0, 'solid.frag.tgsi', 11), (1, 'copy.frag.tgsi', 52)]
    entries += [(int(p), name, int(tokens)) for p, name, tokens in re.findall(
        r'\((\d+), Program \{.*?include_bytes!\("(.*?)"\), tokens: (\d+)\}',
        (directory / 'pipelines/mod.rs').read_text())]
    assert [p for p, _, _ in entries] == list(range(16))
    shaders = [{'name': 'SOLID_VS', 'sha256': hashlib.sha256(vertex).hexdigest()}]
    programs = []
    for pipeline, name, tokens in entries:
        fragment = (directory / 'pipelines' / name).read_bytes()
        shaders.append({'name': name, 'sha256': hashlib.sha256(fragment).hexdigest()})
        programs.append((pipeline, vertex, fragment, tokens))
    return programs, shaders


def function(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


def worker(bundle, output, mode):
    frameworks = bundle / 'Contents/Frameworks'
    egl = C.CDLL(str(frameworks / 'EGL.framework/EGL'), mode=C.RTLD_GLOBAL)
    gles = C.CDLL(str(frameworks / 'GLESv2.framework/GLESv2'), mode=C.RTLD_GLOBAL)
    epoxy = C.CDLL(str(frameworks / 'libepoxy.0.dylib'), mode=C.RTLD_GLOBAL)
    virgl = C.CDLL(str(frameworks / 'libvirglrenderer.1.dylib'))
    proc = function(egl, 'eglGetProcAddress', P, C.c_char_p)
    get_display = C.CFUNCTYPE(P, U, P, C.POINTER(I))(proc(b'eglGetPlatformDisplayEXT'))
    display = get_display(0x3202, None, (I * 3)(0x3203, 0x3489, 0x3038))
    assert display, 'Metal display unavailable'
    initialize = function(egl, 'eglInitialize', U, P, C.POINTER(I), C.POINTER(I))
    assert initialize(display, None, None)
    query = function(egl, 'eglQueryString', C.c_char_p, P, I)
    assert b'EGL_ANDROID_blob_cache' in query(display, 0x3055)
    assert function(egl, 'eglBindAPI', U, U)(0x30A0)
    config, count = P(), I()
    attrs = (I * 7)(0x3033, 1, 0x3040, 0x40, 0x3024, 8, 0x3038)
    assert function(egl, 'eglChooseConfig', U, P, C.POINTER(I), C.POINTER(P), I,
                    C.POINTER(I))(display, attrs, C.byref(config), 1, C.byref(count))
    assert count.value == 1
    create = function(egl, 'eglCreateContext', P, P, P, P, C.POINTER(I))
    destroy = function(egl, 'eglDestroyContext', U, P, P)
    current = function(egl, 'eglMakeCurrent', U, P, P, P, P)
    root = create(display, config, None, (I * 5)(0x3098, 3, 0x30FB, 0, 0x3038))
    assert root and current(display, None, None, root)
    renderer = function(gles, 'glGetString', C.c_char_p, U)(0x1F01).decode()
    assert 'ANGLE' in renderer and 'Metal' in renderer, renderer

    cache = {'gets': 0, 'hits': 0, 'puts': 0, 'bytes_written': 0}
    cache_dir = output / 'blobs'
    cache_dir.mkdir(exist_ok=True)
    callbacks = []
    SET = C.CFUNCTYPE(None, P, C.c_ssize_t, P, C.c_ssize_t)
    GET = C.CFUNCTYPE(C.c_ssize_t, P, C.c_ssize_t, P, C.c_ssize_t)

    @SET
    def put(key, key_size, value, size):
        if not 0 < key_size <= 128 or not 0 < size <= 16 * 1024 * 1024:
            return
        path = cache_dir / C.string_at(key, key_size).hex()
        # The harness callbacks may run on ANGLE's compiler workers.
        temporary = path.with_suffix('.tmp-' + str(threading.get_ident()))
        temporary.write_bytes(C.string_at(value, size))
        temporary.replace(path)
        cache['puts'] += 1
        cache['bytes_written'] += size

    @GET
    def get(key, key_size, value, size):
        cache['gets'] += 1
        if not 0 < key_size <= 128:
            return 0
        path = cache_dir / C.string_at(key, key_size).hex()
        try:
            data = path.read_bytes()
        except FileNotFoundError:
            return 0
        if size >= len(data) and value:
            C.memmove(value, data, len(data))
            cache['hits'] += 1
        return len(data)

    if mode != 'disabled':
        setter = C.CFUNCTYPE(None, P, SET, GET)(proc(b'eglSetBlobCacheFuncsANDROID'))
        setter(display, put, get)
    callbacks += [put, get]

    class Params(C.Structure):
        _fields_ = [('version', I), ('shared', C.c_bool), ('major', I),
                    ('minor', I), ('compat', I)]

    CREATE = C.CFUNCTYPE(P, P, I, C.POINTER(Params))
    DESTROY = C.CFUNCTYPE(None, P, P)
    CURRENT = C.CFUNCTYPE(I, P, I, P)
    FENCE = C.CFUNCTYPE(None, P, U)
    DISPLAY = C.CFUNCTYPE(P, P)

    @CREATE
    def create_context(cookie, scanout, param):
        p = param.contents
        return create(display, config, root if p.shared else None,
                      (I * 5)(0x3098, p.major, 0x30FB, p.minor, 0x3038))

    @DESTROY
    def destroy_context(cookie, context):
        destroy(display, context)

    @CURRENT
    def make_current(cookie, scanout, context):
        return 0 if current(display, None, None, context) else -1

    @FENCE
    def fence(cookie, number):
        pass

    @DISPLAY
    def get_egl_display(cookie):
        return display

    class Callbacks(C.Structure):
        _fields_ = [('version', I), ('fence', FENCE), ('create', CREATE),
                    ('destroy', DESTROY), ('current', CURRENT), ('drm', P),
                    ('context_fence', P), ('server', P), ('display', DISPLAY)]

    cb = Callbacks(4, fence, create_context, destroy_context, make_current,
                   None, None, None, get_egl_display)
    init = function(virgl, 'virgl_renderer_init', I, P, I, C.POINTER(Callbacks))
    assert init(C.c_void_p(1), 1 << 4, C.byref(cb)) == 0
    submit = function(virgl, 'virgl_renderer_submit_cmd', I, P, I, I)
    context_create = function(virgl, 'virgl_renderer_context_create', I, U, U, C.c_char_p)
    context_destroy = function(virgl, 'virgl_renderer_context_destroy', None, U)

    # Observe the exported libepoxy dispatch pointers, forwarding to built ANGLE.
    # Timing includes the synchronous status query that resolves async jobs.
    starts, elapsed, statuses, glsl, counts = {}, {}, {}, {}, {}
    active = None

    def hook(name, restype, argtypes, observe):
        actual = function(gles, name, restype, *argtypes)
        prototype = C.CFUNCTYPE(restype, *argtypes)

        @prototype
        def wrapper(*args):
            return observe(actual, *args)

        C.c_void_p.in_dll(epoxy, 'epoxy_' + name).value = C.cast(wrapper, P).value
        callbacks.append(wrapper)

    def begin(kind, actual, object_id):
        starts[(kind, object_id)] = time.perf_counter_ns()
        actual(object_id)

    def end(kind, expected, actual, object_id, pname, value):
        actual(object_id, pname, value)
        if pname == expected and (kind, object_id) in starts:
            elapsed[kind] = elapsed.get(kind, 0) + (time.perf_counter_ns() -
                                                    starts.pop((kind, object_id))) / 1e6
            counts[kind] = counts.get(kind, 0) + 1
            statuses[kind] = statuses.get(kind, True) and bool(value[0])

    def source(actual, object_id, n, strings, lengths):
        parts = [C.string_at(strings[i], lengths[i]) if lengths and lengths[i] >= 0
                 else C.string_at(strings[i]) for i in range(n)]
        data = b''.join(parts)
        digest = hashlib.sha256(data).hexdigest()
        (output / (digest + '.glsl')).write_bytes(data)
        glsl.setdefault(str(active), []).append(digest)
        actual(object_id, n, strings, lengths)

    hook('glCompileShader', None, [U], lambda fn, obj: begin('compile', fn, obj))
    hook('glGetShaderiv', None, [U, U, C.POINTER(I)],
         lambda fn, obj, pname, value: end('compile', 0x8B81, fn, obj, pname, value))
    hook('glLinkProgram', None, [U], lambda fn, obj: begin('link', fn, obj))
    hook('glGetProgramiv', None, [U, U, C.POINTER(I)],
         lambda fn, obj, pname, value: end('link', 0x8B82, fn, obj, pname, value))
    hook('glShaderSource', None, [U, I, C.POINTER(P), C.POINTER(I)], source)

    def command(ctx, opcode, obj, payload):
        words = [opcode | obj << 8 | len(payload) << 16] + payload
        buffer = (U * len(words))(*words)
        assert submit(buffer, ctx, len(words)) == 0, (ctx, opcode)

    def shader(ctx, handle, kind, text, tokens):
        text = text.rstrip(b'\0') + b'\0'
        total = len(text)
        text += b'\0' * (-total % 4)
        words = list(struct.unpack('<' + 'I' * (len(text) // 4), text))
        for start in range(0, len(words), 1018):
            offset = total if start == 0 else (start * 4) | 0x80000000
            command(ctx, 1, 4, [handle, kind, offset, tokens, 0] + words[start:start + 1018])

    programs, _ = catalogue()
    contexts = []
    for ctx in (1, 2):
        assert context_create(ctx, 7, b'a3-host') == 0
        rows = []
        for pipeline, vertex, fragment, tokens in programs:
            active = pipeline
            elapsed.clear()
            statuses.clear()
            counts.clear()
            start = time.perf_counter_ns()
            shader(ctx, pipeline * 2 + 1, 0, vertex, 11)
            shader(ctx, pipeline * 2 + 2, 1, fragment, tokens)
            command(ctx, 52, 0, [pipeline * 2 + 1, pipeline * 2 + 2, 0, 0, 0, 0])
            assert counts == {'compile': 2, 'link': 1}, counts
            rows.append({'pipeline': pipeline, 'compile_ms': elapsed.get('compile', 0),
                         'link_ms': elapsed.get('link', 0),
                         'submit_ms': (time.perf_counter_ns() - start) / 1e6,
                         'linked': statuses.get('link', False) and statuses.get('compile', False)})
        contexts.append(rows)
        context_destroy(ctx)
    function(virgl, 'virgl_renderer_cleanup', None, P)(C.c_void_p(1))
    current(display, None, None, None)
    destroy(display, root)
    function(egl, 'eglTerminate', U, P)(display)
    return {'mode': mode, 'renderer': renderer, 'cache': cache,
            'contexts': contexts, 'glsl': glsl}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--worker', choices=['disabled', 'populate', 'persistent'])
    parser.add_argument('--bundle', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        result = worker(args.bundle, args.output, args.worker)
        (args.output / (args.worker + '.json')).write_text(json.dumps(result, indent=2) + '\n')
        return
    binary = Path(subprocess.check_output([str(ROOT / 'scripts/build-qemu-gl.sh'),
                                          '--print-path'], text=True).strip())
    bundle = binary.parents[2]
    env = dict(os.environ, ANGLE_DEFAULT_PLATFORM='metal',
               ANGLE_FEATURE_OVERRIDES_ENABLED='', ANGLE_FEATURE_OVERRIDES_DISABLED='',
               DYLD_FRAMEWORK_PATH=str(bundle / 'Contents/Frameworks'))
    if (args.output / 'blobs').exists():
        raise SystemExit('use a new output directory: persistent cache must start empty')
    runs = []
    for mode in ('disabled', 'populate', 'persistent'):
        subprocess.run([sys.executable, __file__, '--worker', mode, '--bundle', str(bundle),
                        '--output', str(args.output)], env=env, check=True)
        runs.append(json.loads((args.output / (mode + '.json')).read_text()))
    _, shaders = catalogue()
    pins = (bundle / 'Contents/Resources/licenses/PINS').read_text()
    result = {'pins': pins, 'shaders': shaders, 'runs': runs,
              'library_sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                                 for name, path in {
                                     'EGL': bundle / 'Contents/Frameworks/EGL.framework/EGL',
                                     'GLESv2': bundle / 'Contents/Frameworks/GLESv2.framework/GLESv2',
                                     'virglrenderer': bundle / 'Contents/Frameworks/libvirglrenderer.1.dylib'
                                 }.items()}}
    (args.output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
    print(args.output / 'results.json')


if __name__ == '__main__':
    main()
