#!/usr/bin/env python3
"""Check Expect claims against candidate pixels without making approval decisions.

Coordinates are zero-based, top-left; (x,y)-(right,bottom) is half-open.
Unknown prose is reported, never treated as proof. Intent bands preserve source pixels;
paged captures judge only the fixtures present on that page. Requires Pillow, as does approve.py.
"""
import argparse
import ast
from collections import Counter
from functools import lru_cache
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import re
import sys
import subprocess

from PIL import Image, ImageChops, ImageDraw

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'metal'))
import intent_manifest  # noqa: E402

HEX = re.compile(r'#[0-9A-Fa-f]{6}\b')
SIZE = re.compile(r'\b(\d+)x(\d+)\b')
POINT = re.compile(r'\((-?\d+),\s*(-?\d+)\)(?:\s*[-–]\s*\((-?\d+),\s*(-?\d+)\))?')
ROOT = Path(__file__).resolve().parents[2]
IMAGES = 'userspace/programs/src/suite_breengel/images.rs'
CHECKS = 'libs/libimg/tests/checks.rs'


@lru_cache(maxsize=256)
def source_bytes(path, commit=None):
    """Use the captured case's code, never a decoder or the candidate as oracle."""
    if commit is None:
        return (ROOT / path).read_bytes()
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('invalid fixture source commit')
    result = subprocess.run(['git', 'show', f'{commit}:{path}'], cwd=ROOT, capture_output=True)
    if result.returncode:
        raise ValueError(f'fixture source unavailable: {commit}:{path}')
    return result.stdout


def function_body(text, name):
    start = re.search(r'\bfn ' + re.escape(name) + r'\([^\n]*', text)
    if not start:
        raise ValueError(f'fixture function missing: {name}')
    begin = text.index('{', start.start())
    level = 1
    for end in range(begin+1, len(text)):
        level += (text[end] == '{') - (text[end] == '}')
        if level == 0:
            return text[begin+1:end]
    raise ValueError(f'unterminated fixture function: {name}')


def literal_arrays(body):
    return [ast.literal_eval('[' + m[1] + ']') for m in
            re.finditer(r'&\[([\d,\s]+)\]', body)]


def panel_fixtures(case, commit=None):
    """Independent BGRA reference models of the recorded fixtures, in tile order.

    Literal arrays are read from the case source. Procedural fixtures model its
    documented sample arithmetic; external fixtures use their committed BGRA,
    not a PNG decode. The returned source hashes bind these checks to that code.
    """
    sources = {}

    def read(path):
        data = source_bytes(path, commit)
        sources[path] = hashlib.sha256(data).hexdigest()
        return data

    images = read(IMAGES).decode()
    checks = read(CHECKS).decode()
    fixtures = []

    def add(w, h, pixels, label=None):
        pixels = bytes(pixels)
        if len(pixels) != w*h*4:
            raise ValueError('fixture pixel length mismatch')
        fixtures.append(dict(width=w, height=h, pixels=pixels, label=label or f'{w}x{h}'))

    def scene(w, h, label=None):
        body = function_body(checks, 'scene')
        if '[(x * 23) as u8, (y * 19) as u8, ((x + y) * 7) as u8, 255]' not in body:
            raise ValueError('unsupported scene fixture arithmetic')
        add(w, h, (v for y in range(h) for x in range(w)
                   for v in (x*23 % 256, y*19 % 256, (x+y)*7 % 256, 255)), label)

    def rng_fixture(w, h, seed, label):
        rng_source = read('userspace/programs/src/suite_breengel/main.rs').decode()
        if not all(s in rng_source for s in ('self.0 >> 12', 'self.0 << 25', 'self.0 >> 27', '0x2545_F491_4F6C_DD1D')):
            raise ValueError('unsupported BMP fixture generator')
        state = seed | 1
        mask = (1 << 64)-1
        def next_byte():
            nonlocal state
            state ^= state >> 12
            state ^= state << 25 & mask
            state ^= state >> 27
            return (state * 0x2545F4914F6CDD1D & mask) % 256
        pixels = []
        for _ in range(w*h):
            r, g, b = next_byte(), next_byte(), next_byte()
            pixels.extend((b,g,r,255))
        add(w,h,pixels,label)

    scale = lambda v, d: (v*255+32767)//65535 if d == 16 else v*255//((1 << d)-1)
    pm = lambda c, a: (c*a+127)//255
    name = case.replace('-', '_').replace('/', '_')
    if case == 'bmp/decode-fixtures':
        body = function_body(images, 'bmp_decode_fixtures_checks')
        rows = re.findall(r'let rows = (.*?);', body, re.S)
        labels = ['a 3x2 fixture', "the contract's 2x2 file"]
        if len(rows) != 2:
            raise ValueError('unsupported BMP literal inventory')
        for value, label in zip(rows, labels):
            rows_rgb = ast.literal_eval(value.replace('vec!', ''))
            add(len(rows_rgb[0]), len(rows_rgb), (v for row in rows_rgb for r,g,b in row for v in (b,g,r,255)), label)
    elif case == 'bmp/orientation':
        for label in ['a bottom-up 5x4 BMP', 'the same pixels top-down (negative height)']:
            rng_fixture(5,4,0xB0B,label)
    elif case == 'bmp/padding':
        for w in range(1,5):
            rng_fixture(w,3,w*77,f'a {w}x3 BMP')
    elif case == 'bmp/roundtrip':
        for w,h in [(1,1),(5,3),(7,7),(64,2)]:
            rng_fixture(w,h,w*31+h,f'an encoded {w}x{h} image')
    elif case == 'bmp/truncated':
        rng_fixture(3,3,0x7A7,'the whole 3x3 file')
    elif case in ('png-decode/gray-depths', 'png-decode/gray-alpha', 'png-decode/rgb', 'png-decode/rgba'):
        kind = {'gray-depths':0, 'gray-alpha':4, 'rgb':2, 'rgba':6}[case.split('/')[1]]
        for d in ([1,2,4,8,16] if kind == 0 else [8,16]):
            m = (1 << d)-1
            values = {0:[(0,0,0,m),(m//2,m//2,m//2,m),(m,m,m,m)],
                      4:[(m,m,m,m//2),(m//2,m//2,m//2,m)],
                      2:[(0,m//2,m,m),(m,0,m//2,m)],
                      6:[(m,m//2,0,m//2),(0,m,m,m)]}[kind]
            pixels = []
            for r,g,b,a in values:
                a = scale(a,d)
                pixels.extend((pm(scale(b,d),a),pm(scale(g,d),a),pm(scale(r,d),a),a))
            add(len(values),1,pixels)
    elif case == 'png-decode/palette-depths':
        pixels = literal_arrays(function_body(checks,name))[-1]
        for _ in range(4):
            add(4,1,pixels)
    elif case == 'png-decode/trns-gray':
        for d in [1,2,4,8,16]:
            if d < 16:
                add(2,1,[0,0,0,255,0,0,0,0])
            c = scale(1,d)
            add(2,1,[0,0,0,0,c,c,c,255])
    elif case == 'png-decode/trns-rgb':
        body = function_body(checks,name)
        add(2,1,literal_arrays(body)[1])
        for d in [8,16]:
            add(2,1,[0,0,0,0,scale(1,d),scale(2,d),scale(3,d),255])
    elif case == 'png-decode/trns-palette':
        add(3,1,literal_arrays(function_body(checks,name))[-1])
    elif case == 'png-decode/rounding-16bit':
        body = function_body(checks,name)
        values = ast.literal_eval('[' + re.search(r'let vals = \[([\d,\s]+)\]',body)[1] + ']')
        add(len(values),1,(v for n in values for v in (scale(n,16),)*3+(255,)))
    elif case == 'png-decode/filters':
        for t,d,ch in [(0,8,1),(0,16,1),(2,8,3),(2,16,3),(4,8,2),(4,16,2),(6,8,4),(6,16,4)]:
            bpp = ch*(d//8)
            row = [(i*37+17)%256 for i in range(7*bpp)]
            pixels = []
            for x in range(7):
                raw = row[x*bpp:(x+1)*bpp]
                v = [raw[i]*256+raw[i+1] for i in range(0,len(raw),2)] if d == 16 else raw
                m = (1 << d)-1
                r,g,b,a = {0:(v[0],v[0],v[0],m), 2:(v[0],v[min(1,ch-1)],v[min(2,ch-1)],m),
                           4:(v[0],v[0],v[0],v[-1]), 6:(v[0],v[min(1,ch-1)],v[min(2,ch-1)],v[-1])}[t]
                a = scale(a,d)
                pixels.extend((pm(scale(b,d),a),pm(scale(g,d),a),pm(scale(r,d),a),a))
            add(7,5,pixels*5)
    elif case in ('png-decode/interlace','png-decode/interlace-small','png-encode/stable','png-encode/sizes'):
        sizes = {'png-decode/interlace':[(17,19)], 'png-decode/interlace-small':[(w,h) for w in range(1,10) for h in range(1,10)],
                 'png-encode/stable':[(13,7)], 'png-encode/sizes':[(1,1),(3,7),(17,9),(1280,800)]}[case]
        for w,h in sizes:
            scene(w,h)
    elif case in ('png-decode/multi-idat','png-decode/chunk-order'):
        add(1,1,literal_arrays(function_body(checks,name))[-1])
    elif case == 'png-decode/ancillary-ignored':
        body = function_body(checks,name)
        for match in re.finditer(r'\(\d, vec!\[[^]]*\], vec!\[[^]]*\], vec!\[([\d,\s]+)\]\)',body):
            add(1,1,ast.literal_eval('['+match[1]+']'))
        arrays = [a for a in literal_arrays(body) if len(a) == 4]
        add(1,1,arrays[-2])
        add(1,1,arrays[-1])
        if len(fixtures) != 6:
            raise ValueError('unsupported ancillary literal inventory')
    elif case in ('png-decode/independent-fixtures','png-decode/from-disk'):
        base = 'testdata/breengel/png/'
        entries = [line.split() for line in read(base+'index.txt').decode().splitlines()] if case.endswith('independent-fixtures') else [['pinned.png','32','32','pinned.bgra']]
        for png,w,h,bgra in entries:
            add(int(w),int(h),read(base+bgra),png if case.endswith('independent-fixtures') else None)
    elif case == 'png-decode/blit':
        body = function_body(images,'png_decode_blit_checks')
        pixels = ast.literal_eval('['+re.search(r'let expected = \[([\d,\s]+)\]',body)[1]+']')
        add(3,1,pixels,'source-over')
    elif case == 'png-encode/decodes':
        body = function_body(checks,name)
        for match in re.finditer(r'vec!\[([\d,\s]+)\]',body):
            add(2,1,ast.literal_eval('['+match[1]+']'))
    else:
        raise ValueError(f'unsupported comparison panel case: {case}')
    return fixtures, sources


def panel_layout(fixtures, target, header=0):
    """Reconstruct tile locations from fixture inventory, never picture colours."""
    tiles = []
    for f in fixtures:
        w,h = f['width'],f['height']
        longest = max(w,h,1)
        scale, divisor = max(target//longest,1), (longest+target-1)//target
        pw,ph = (w+divisor-1)//divisor*scale, (h+divisor-1)//divisor*scale
        content = max(2*(pw+2)+8, min(len(f['label'])*6,512), len(f'difference shown at 1/{divisor}')*6 if divisor > 1 else 60)
        title_h = (len(f['label'])+content//6-1)//(content//6)*12
        tiles.append(dict(width=content+16,height=16+title_h+3*(16+ph+2+8),
                          picture_w=pw,picture_h=ph,scale=scale,divisor=divisor,title_height=title_h))
    columns = 1
    while columns*columns*sum(t['width'] for t in tiles) < len(tiles)*sum(t['height'] for t in tiles):
        columns += 1
    columns = min(columns,len(tiles))
    widths = [max(t['width'] for t in tiles[c::columns]) for c in range(columns)]
    y = header
    for start in range(0,len(tiles),columns):
        row = tiles[start:start+columns]
        title_h = max(t['title_height'] for t in row)
        height = max(t['height']+title_h-t['title_height'] for t in row)
        x = 0
        for column,t in enumerate(row):
            t.update(x=x,y=y,title_height=title_h)
            x += widths[column]
        y += height
    return (sum(widths),y), tiles


def panel_plan(fixtures):
    for target in (256,192,128,96,64):
        size,tiles = panel_layout(fixtures,target)
        if size[0]*size[1] <= 5*1024*1024:
            return size,tiles,len(fixtures),1
    end = 1
    size,tiles = panel_layout(fixtures[:end],64,24)
    while end < len(fixtures):
        next_size,next_tiles = panel_layout(fixtures[:end+1],64,24)
        if next_size[0]*next_size[1] > 5*1024*1024:
            break
        size,tiles = next_size,next_tiles
        end += 1
    return size,tiles,end,None


def verify_panel(image, expect, case, checkpoint, commit=None):
    result = dict(verdict='unparseable', panel=dict(case=case), checked_claims=[],failed_claims=[],unparseable_claims=[])
    def check(kind, claim, expected, measured, ok):
        entry = dict(kind=kind,claim=claim,expected=expected,measured=measured,verdict='pass' if ok else 'fail')
        result['checked_claims'].append(entry)
        if not ok:
            result['failed_claims'].append(entry)
    def unknown(claim, reason):
        entry = dict(claim=claim,reason=reason)
        if entry not in result['unparseable_claims']:
            result['unparseable_claims'].append(entry)
    try:
        fixtures,sources = panel_fixtures(case,commit)
        size,tiles,shown,pages = panel_plan(fixtures)
        is_intent = checkpoint.startswith('intent')
        if is_intent:
            row = next(r for r in intent_manifest.inventory() if r['suite']=='breengel' and r['case']==case)
            if tuple(row['size']) != tuple(size):
                raise ValueError('intent inventory differs from production panel dimensions')
            restored,page,page_count,mismatch,scale_bad = restore_intent(image,size,checkpoint,intent_manifest.cells(row))
            check('layout','intent checkerboard and 2 px white border',list(page['box']),dict(mismatch_box=mismatch),mismatch is None)
            check('scale','integer enlargement blocks',page['scale'],dict(mismatch_box=scale_bad),scale_bad is None)
        else:
            restored=image
            page=dict(box=(0,0,*size),scale=1,tiles=[((0,0,*size),(0,0,*size))])
            page_count=1
            check('layout','panel dimensions',list(size),list(image.size),size == image.size)
        result['picture'] = dict(size=list(size),source_size=list(size),box=list(page['box']),scale=page['scale'],divisor=1)
        result['panel'].update(source_commit=commit,source_sha256=sources,fixtures=len(fixtures),pages=page_count,tiles=[])
        if page_count > 1:
            unknown('page scope','this checkpoint proves only its whole fixture tiles; companion pages are separate approvals')
        def screen_box(rect):
            x,y,r,b = rect
            for (dx,dy,dw,dh),(sx,sy,sw,sh) in page['tiles']:
                if sx <= x and sy <= y and r <= sx+sw and b <= sy+sh:
                    scale=dw//sw
                    return (dx+(x-sx)*scale,dy+(y-sy)*scale,dx+(r-sx)*scale,dy+(b-sy)*scale)
            raise ValueError('panel rectangle crosses an intent tile')
        def crop(rect):
            screen_box(rect)
            return restored.crop(rect)
        for index,(f,t) in enumerate(zip(fixtures,tiles)):
            if not any(sx <= t['x'] < sx+sw and sy <= t['y'] < sy+sh
                       for _,(sx,sy,sw,sh) in page['tiles']):
                continue
            label = f'tile {index}: {f["label"]}'
            td,ts = t['divisor'],t['scale']
            w,h = (f['width']+td-1)//td,(f['height']+td-1)//td
            tile_record = dict(index=index,label=f['label'],source_size=[f['width'],f['height']],scale=ts,divisor=td,pictures=[])
            result['panel']['tiles'].append(tile_record)
            if td > 1:
                unknown(label,f'tile decimation by {td} omits fixture pixels; only visible samples are checked')
            pictures = []
            scale_bad, border_bad = [],[]
            for row in range(3):
                pair = []
                for column in range(2):
                    x = t['x']+8+column*(t['picture_w']+10)
                    y = t['y']+8+t['title_height']+row*(26+t['picture_h'])+16
                    pw,ph = t['picture_w'],t['picture_h']
                    tile_record['pictures'].append(dict(row=row,plane='colour' if column == 0 else 'alpha',box=list(screen_box((x+1,y+1,x+1+pw,y+1+ph)))))
                    for edge in ((x,y,x+pw+2,y+1),(x,y+ph+1,x+pw+2,y+ph+2),(x,y,x+1,y+ph+2),(x+pw+1,y,x+pw+2,y+ph+2)):
                        area = crop(edge)
                        if area is not None and ImageChops.difference(area,Image.new('RGB',area.size,'white')).getbbox():
                            border_bad.append([row,column,list(edge)])
                    if row == 2:
                        area = crop((x+1,y+1,x+pw+1,y+ph+1))
                        mismatch = area.getbbox() if area else None
                        check('panel-difference',f'{label} {"colour" if column == 0 else "alpha"} difference entirely black',
                              '#000000',dict(mismatch_box=mismatch),area is not None and mismatch is None)
                        continue
                    values = []
                    for sy in range(h):
                        for sx in range(w):
                            area = crop((x+1+sx*ts,y+1+sy*ts,x+1+(sx+1)*ts,y+1+(sy+1)*ts))
                            if area is None:
                                values.append(None)
                                unknown(label,'some fixture samples are omitted by intent decimation')
                                continue
                            value = area.getpixel((0,0))
                            if ImageChops.difference(area,Image.new('RGB',area.size,value)).getbbox():
                                scale_bad.append([row,column,sx,sy])
                            values.append(value)
                    pair.append(values)
                pictures.append(pair)
            check('panel-border',f'{label} white picture outlines','all visible border pixels white',dict(mismatches=border_bad[:8],count=len(border_bad)),not border_bad)
            check('panel-scale',f'{label} integer enlargement blocks','uniform blocks in both planes',dict(mismatches=scale_bad[:8],count=len(scale_bad)),not scale_bad)
            for column in range(2):
                actual,reference = pictures[0][column],pictures[1][column]
                compared = sum(a is not None and r is not None for a,r in zip(actual,reference))
                bad = [i for i,(a,r) in enumerate(zip(actual,reference)) if a is not None and r is not None and a != r]
                check('panel-pair',f'{label} {"colour" if column == 0 else "alpha"} actual equals reference','pixel equality',
                      dict(compared_pixels=compared,mismatches=bad[:8],count=len(bad)),compared > 0 and not bad)
                bad = []
                compared = 0
                for sy in range(h):
                    for sx in range(w):
                        value = reference[sy*w+sx]
                        if value is None:
                            continue
                        at = ((sy*td)*f['width']+sx*td)*4
                        b,g,r,a = f['pixels'][at:at+4]
                        expected = (r,g,b) if column == 0 else (a,a,a)
                        compared += 1
                        if value != expected:
                            bad.append(dict(x=sx*td,y=sy*td,expected=expected,measured=value))
                check('panel-reference',f'{label} {"colour" if column == 0 else "alpha"} reference equals fixture BGRA',
                      dict(fixture_sha256=hashlib.sha256(f['pixels']).hexdigest()),dict(compared_pixels=compared,mismatches=bad[:8],count=len(bad)),compared > 0 and not bad)
        unknown(expect or 'expect','labels, glyphs and remaining visual prose require independent review; mechanical evidence alone is not G29 approval')
    except (ValueError, OSError, IndexError, AttributeError, SyntaxError) as exc:
        check('panel-input','source-derived panel inventory and layout','supported fixture source',str(exc),False)
    result['verdict'] = 'fail' if result['failed_claims'] else 'unparseable' if result['unparseable_claims'] else 'pass'
    return result


def rgb(value):
    return tuple(bytes.fromhex(value.lstrip('#')))


def colour_name(value):
    return '#%02X%02X%02X' % value


def layout(size, mode):
    pages = intent_manifest.plans(size,mode)
    if len(pages) != 1:
        raise ValueError('intent requires numbered pages')
    return pages[0]['box'], pages[0]['scale'], 1


def restore_intent(image, size, checkpoint, source_cells=()):
    pages = intent_manifest.plans(size,image.size,source_cells)
    if len(pages) == 1:
        if checkpoint != 'intent':
            raise ValueError('unpaged intent has a page suffix')
        page = pages[0]
    else:
        match = re.fullmatch(r'intent-p([1-9][0-9]*)',checkpoint)
        if not match or int(match[1]) > len(pages):
            raise ValueError('intent requires a valid numbered page')
        page = pages[int(match[1])-1]
    template = checker(image.size)
    if page['border']:
        x,y,r,b = page['border']
        ImageDraw.Draw(template).rectangle((x,y,r-1,b-1),fill='white')
        ImageDraw.Draw(template).rectangle((x+2,y+2,r-3,b-3),fill='#0C131F')
    picture = Image.new('RGB',size)
    scale_mismatch = None
    for (dx,dy,dw,dh),(sx,sy,sw,sh) in page['tiles']:
        block = image.crop((dx,dy,dx+dw,dy+dh))
        template.paste(block,(dx,dy))
        original = block.resize((sw,sh),Image.Resampling.NEAREST)
        mismatch = ImageChops.difference(block,original.resize(block.size,Image.Resampling.NEAREST)).getbbox()
        if mismatch:
            scale_mismatch = mismatch
        picture.paste(original,(sx,sy))
    return picture, page, len(pages), ImageChops.difference(image,template).getbbox(), scale_mismatch


def checker(mode):
    image = Image.new('RGB', mode, '#5A5A5A')
    draw = ImageDraw.Draw(image)
    for y in range(0, mode[1], 16):
        for x in range(0, mode[0], 16):
            if (x // 16 + y // 16) % 2:
                draw.rectangle((x, y, x + 15, y + 15), fill='#7A7A7A')
    return image


def dimension_claims(text):
    """Source dimensions must describe a surface, not a font or object size."""
    found = []
    for match in SIZE.finditer(text):
        before, after = text[:match.start()], text[match.end():]
        surface = re.match(r'\s+(?:(?:[a-z-]+\s+){0,3}(?:\(#[0-9a-fA-F]{6}\)\s+)?)?'
                           r'(?:picture|background|field|patch|rectangle|square|strip|scene|grid)\b', after)
        explicit = re.search(r'(?:picture is|judged crop is|end screen.*across)\s*$', before)
        object_at = surface and re.match(r'\s+(?:at|from|starts at) \(', after[surface.end():])
        interior = ('. ' in before or 'labels head' in before or
                    re.search(r'form a\s*$', before) or
                    re.match(r'\s+(?:mosaic|coloured) rectangles', after))
        if (surface and not object_at and not interior) or explicit:
            found.append((tuple(map(int, match.groups())), match.span()))
    return found


def verify_picture(image, expect, checkpoint='intent', *, case=None, suite=None, commit=None):
    image = image.convert('RGB')
    # Comparison panels describe interior fixture sizes, not the outer picture.
    # Check them before the generic prose grammar can mistake those dimensions.
    if suite == 'breengel' and case and case.split('/')[0] in ('bmp', 'png-decode', 'png-encode'):
        return verify_panel(image, expect, case, checkpoint, commit)
    # Literal scene declarations describe the retained composed end surface.
    scene = re.search(r'seed mosaics \(x,y,width,height,z,seed\): '
                      r'(for i=0\.\.15, \(40\+60\*i,30\+40\*i,160,120,15-i,10\+i\); )?'
                      r'([0-9(),; ]+)\.', expect)
    if scene:
        windows = [(40+60*i,30+40*i,160,120,15-i,10+i) for i in range(16)] if scene[1] else []
        windows += [tuple(map(int, values.split(','))) for values in re.findall(r'\(([0-9,]+)\)', scene[2])]
        size = re.search(r'(\d+)x(\d+) screen', expect)
        if not size or not windows or any(len(w) != 6 for w in windows):
            return dict(verdict='unparseable', checked_claims=[], failed_claims=[],
                        unparseable_claims=[dict(claim=expect, reason='invalid seed scene')])
        width,height = map(int,size.groups())
        reference = Image.new('RGB',(width,height),'black')
        for x,y,w,h,z,seed in sorted(windows,key=lambda w:w[4]):
            tile = Image.new('RGB',(w,h))
            tile.putdata([((px ^ py ^ seed)&255,(5*py+29*seed)&255,(7*px+13*seed)&255)
                          for py in range(h) for px in range(w)])
            reference.paste(tile,(x,y))
        mismatch = None if image.size == reference.size else 'dimensions'
        if mismatch is None:
            mismatch = ImageChops.difference(image,reference).getbbox()
        claim = dict(kind='seed-scene',claim=expect,expected=dict(size=[width,height],windows=windows),
                     measured=dict(size=list(image.size),mismatch_box=mismatch),
                     verdict='fail' if mismatch else 'pass')
        return dict(verdict=claim['verdict'],checked_claims=[claim],
                    failed_claims=[claim] if mismatch else [],unparseable_claims=[])
    result = dict(verdict='unparseable', picture=dict(size=list(image.size),
                  box=[0, 0, *image.size], scale=1, divisor=1),
                  checked_claims=[], failed_claims=[], unparseable_claims=[])
    checks, unknown = result['checked_claims'], result['unparseable_claims']

    def check(kind, claim, expected, measured, ok):
        entry = dict(kind=kind, claim=claim, expected=expected, measured=measured,
                     verdict='pass' if ok else 'fail')
        checks.append(entry)
        if not ok:
            result['failed_claims'].append(entry)

    def unresolved(claim, reason):
        unknown.append(dict(claim=claim, reason=reason))

    if not isinstance(expect, str) or not expect.strip():
        unresolved('expect', 'missing Expect sentence')
        return result
    dimensions = dimension_claims(expect)
    # Largest surface is the picture, while smaller stated sizes can be objects.
    end_size = re.search(r'end screen.*?(?:across|fills) (\d+)x(\d+)', expect)
    judged_size = re.search(r'judged crop is (\d+)x(\d+)', expect)
    if checkpoint == 'end' and end_size:
        source = tuple(map(int, end_size.groups()))
    elif checkpoint.startswith('intent') and judged_size:
        source = tuple(map(int, judged_size.groups()))
    else:
        source = max((item[0] for item in dimensions), key=lambda s: s[0]*s[1], default=None)
    picture = image
    row = next((r for r in intent_manifest.inventory() if r['suite']==suite and r['case']==case),None)
    if checkpoint.startswith('intent') and row:
        source = row['size']
    if checkpoint.startswith('intent') and source and source != image.size:
        try:
            picture,page,page_count,mismatch,scale_bad = restore_intent(image,source,checkpoint,intent_manifest.cells(row) if row else ())
            check('layout','intent checkerboard and 2 px white border',list(page['box']),dict(mismatch_box=mismatch),mismatch is None)
            check('scale','integer nearest-neighbour picture blocks',page['scale'],dict(mismatch_box=scale_bad),scale_bad is None)
            result['picture'] = dict(size=list(picture.size),source_size=list(source),box=list(page['box']),scale=page['scale'],divisor=1)
            if page_count > 1:
                unresolved('page scope','whole-picture prose needs companion pages; this checkpoint is a separate approval')
                # Do not run whole-picture colour or geometry claims on absent pixels.
                result['verdict'] = 'fail' if result['failed_claims'] else 'unparseable'
                return result
        except ValueError as exc:
            check('layout','intent dimensions',list(source),str(exc),False)
    elif checkpoint == 'intent' and not source:
        # A border may surround a picture whose source dimensions the prose omits.
        # Do not count the border/checker colours as picture colours.
        mask = ImageChops.difference(image, Image.new('RGB', image.size, 'white'))
        channels = mask.split()
        equal = ImageChops.lighter(ImageChops.lighter(channels[0], channels[1]), channels[2]).point(lambda v: 255 if v == 0 else 0)
        border = equal.getbbox()
        if border and border[0] >= 30 and border[1] >= 30 and border[2] <= image.width-30 and border[3] <= image.height-30:
            box = (border[0]+2, border[1]+2, border[2]-2, border[3]-2)
            if box[2] > box[0] and box[3] > box[1]:
                template = checker(image.size)
                x,y,right,bottom = box
                ImageDraw.Draw(template).rectangle((x-2,y-2,right+1,bottom+1), fill='white')
                template.paste(image.crop(box),(x,y))
                if ImageChops.difference(image,template).getbbox() is None:
                    picture = image.crop(box)
                    result['picture'].update(size=list(picture.size), box=list(box),
                                            source_dimensions='not specified; displayed crop only')
                    unresolved('picture dimensions', 'verified border but source dimensions and integer scale are not specified')

    if source:
        if checkpoint == 'end' and source != image.size and re.search(r'(?:top left|top-left|screen origin).*charcoal|charcoal.*screen', expect, re.I):
            # The sentence explicitly describes a patch in a larger end frame.
            check('dimensions', 'top-left patch fits end frame', list(source), list(image.size),
                  source[0] <= image.width and source[1] <= image.height)
            unresolved('patch dimensions', 'end frame has no intent border; patch extent requires its shape or background claims')
        else:
            check('dimensions', 'picture dimensions', list(source), list(picture.size), source == picture.size)
    elif not re.search(r'full screen|stage|screen', expect, re.I):
        unresolved('picture dimensions', 'no unambiguous picture dimensions in Expect')

    counts = Counter(dict((colour, count) for count, colour in picture.getcolors(picture.width * picture.height)))
    total = picture.width * picture.height
    # Source blend endpoints describe the formula inputs, not colors that must
    # appear in its output. Validate the stated B,G,R blend output pixels instead.
    presence_text = expect
    blend = re.search(r'B,G,R coverage is x, 255-x and \(([0-9]+)\*x modulo 256\), '
                      r'blended between [^;]+', expect)
    if blend:
        endpoints = list(HEX.finditer(blend[0]))
        patch = re.search(r'(\d+)x(\d+) (?:picture|strip)', expect)
        if len(endpoints) == 2 and patch:
            width, height = map(int, patch.groups())
            back, front = (rgb(v[0]) for v in endpoints)
            extent = width == 256 and 0 < height <= image.height
            expected = Image.new('RGB', (256, height)) if extent else None
            if expected is not None:
                for x in range(256):
                    alpha = ((int(blend[1])*x)%256, 255-x, x)
                    pixel = tuple((front[c]*alpha[c]+back[c]*(255-alpha[c])+127)//255
                                  for c in range(3))
                    for y in range(height): expected.putpixel((x,y), pixel)
            area = picture.crop((0,0,width,height)) if extent and picture.width >= width else None
            mismatch = ImageChops.difference(area, expected).getbbox() if area is not None else 'extent'
            check('blend', blend[0], dict(size=[width,height], rounding='nearest /255'),
                  dict(mismatch_box=mismatch), mismatch is None)
            presence_text = expect[:blend.start()]+expect[blend.end():]
    mosaic = re.search(
        r'seed k is the captured nonzero commit id '
        r'\(for k=1 the upper-left pixel is dark green \((#[0-9A-Fa-f]{6})\)\), '
        r'with B=\(7\*x\+13\*k\)%256, G=\(5\*y\+29\*k\)%256, '
        r'R=\(x XOR y XOR k\)%256', expect)
    if mosaic:
        # The example describes k=1. The blue origin gives
        # k modulo 256 because 197 is the inverse of 13 modulo 256. Check
        # the picture's B,G,R channels against that one common residue.
        seed = (picture.getpixel((0, 0))[2] * 197) % 256
        expected = Image.new('RGB', picture.size)
        expected.putdata([((x ^ y ^ seed) % 256, (5*y+29*seed) % 256,
                           (7*x+13*seed) % 256)
                          for y in range(picture.height) for x in range(picture.width)])
        mismatch = ImageChops.difference(picture, expected).getbbox()
        check('mosaic', 'one common commit residue satisfies all B,G,R formulas',
              dict(formula=mosaic[0]),
              dict(seed_modulo_256=seed, compared_pixels=total, mismatch_box=mismatch),
              mismatch is None)
        check('mosaic-example', 'k=1 upper-left example', '#011D0D', mosaic[1].upper(),
              rgb(mosaic[1]) == (1, 29, 13))
        unresolved('captured commit id', 'pixel formula identifies its residue only; the live case checks commit identity')
        presence_text = presence_text.replace(mosaic[0], '', 1)
    colours = list(dict.fromkeys(m[0].upper() for m in HEX.finditer(presence_text)))
    for colour in colours:
        count = counts[rgb(colour)]
        check('colour', f'{colour} present', dict(minimum_pixels=1),
              dict(pixels=count, total=total, share=count/total), count > 0)

    # A text origin is a layout anchor, not a promised glyph pixel. This checks
    # local ink only; the string, font and glyph shapes still need vision review.
    text_claims = list(re.finditer(
        r"the text '([^']+)', [a-z -]+ \(?(#[0-9a-fA-F]{6})\)?, "
        r'starting near x=(-?\d+), y=(-?\d+), font size (\d+(?:\.\d+)?)', expect, re.I))
    for claim in text_claims:
        text, colour, x, y, size = claim.groups()
        x, y, size = int(x), int(y), float(size)
        if size <= 0 or size > 16384:
            check('text-colour', claim[0], 'positive bounded font size', size, False)
            continue
        # Allow bearings around the anchor and up to 1.5 em advance per codepoint.
        # Bounds are computed from the stated text/size before measuring the ink.
        pad = math.ceil(size / 4)
        box = [max(0, x-pad), max(0, y-pad),
               min(picture.width, x+math.ceil(len(text)*size*1.5)+pad),
               min(picture.height, y+math.ceil(size*1.5)+pad)]
        valid = box[0] < box[2] and box[1] < box[3]
        ink = picture.crop(box).getcolors(picture.width*picture.height) if valid else ()
        count = sum(n for n, pixel in ink if pixel == rgb(colour))
        check('text-colour', claim[0], dict(colour=colour.upper(), minimum_pixels=1),
              dict(box=box, pixels=count), count > 0)
    for background in re.finditer(r'on [a-z -]+ \(?(#[0-9a-fA-F]{6})\)? background', expect, re.I):
        colour = background[1].upper()
        count = counts[rgb(colour)]
        check('background', background[0], dict(colour=colour, share_greater_than=0.5),
              dict(pixels=count, total=total, share=count/total), count*2 > total)

    # Filled single-colour surfaces make the absence of text/marks mechanically provable.
    filled = re.search(r'\b(entirely|solid|plain|uniform)\b', expect, re.I)
    exclusive = re.search(r'\b(?:only|nothing else|no other colou?rs?)\b', expect, re.I)
    palette = colours
    if filled and colours:
        # A solid local object on a contrasting background is not a solid picture.
        first_point = POINT.search(expect)
        local = first_point and first_point.start() < expect.find(colours[-1])
        if len(colours) == 1 and not local:
            share = counts[rgb(colours[0])] / total
            check('palette', 'entire picture has the stated uniform colour', colours,
                  dict(pixels=counts[rgb(colours[0])], total=total, share=share), share == 1)
        else:
            unresolved(filled[0], 'uniformity describes a local or historical object; its extent is not unambiguous')
    elif exclusive and colours:
        if re.search(r'containing only|(?:colou?rs?|pixels) (?:are )?only|nothing else', expect, re.I):
            covered = sum(counts[rgb(c)] for c in palette)
            check('palette', 'only named colours occur', palette,
                  dict(pixels=covered, total=total, share=covered/total,
                       other_pixels=total-covered), covered == total)
        else:
            unresolved(exclusive[0], 'exclusive prose is not an unambiguous colour palette')

    regions = []
    points = list(POINT.finditer(expect))
    for point in points:
        before = expect[:point.start()]
        colour_matches = list(HEX.finditer(before))
        nearest = colour_matches[-1] if colour_matches else None
        context_start = max(before.rfind(';'), before.rfind('. ')) + 1
        context = before[context_start:]
        if (not nearest or nearest.start() < context_start or
                re.search(r'text|glyph|font|outline|ring|circle|triangle|diagonal|arrow|pointer|noise|mosaic|ramp|near|from|texture coordinates', context, re.I)):
            unresolved(point[0], 'coordinate describes geometry/text/source or has no unambiguous literal colour')
            continue
        between = before[nearest.end():]
        # An intervening colour word without a literal cannot borrow the previous literal.
        if re.search(r'\b(?:red|green|blue|white|black|violet|cyan)\b', between, re.I):
            unresolved(point[0], 'coordinate colour is not an adjacent explicit #RRGGBB')
            continue
        x, y = map(int, point.group(1, 2))
        if point[3] is not None:
            right, bottom = map(int, point.group(3, 4))
        else:
            sizes = list(SIZE.finditer(between))
            following_size = re.match(r',?\s*size (\d+)x(\d+)', expect[point.end():])
            size = following_size or (sizes[-1] if sizes else None)
            w, h = map(int, size.groups()) if size else (1, 1)
            right, bottom = x+w, y+h
        region = [x, y, right, bottom]
        colour = nearest[0].upper()
        regions.append((region, colour, expect[nearest.start():point.end()]))

    for index, (region, colour, claim) in enumerate(regions):
        x, y, right, bottom = region
        in_bounds = 0 <= x < right <= picture.width and 0 <= y < bottom <= picture.height
        measured = dict(box=region, in_bounds=in_bounds)
        ok = False
        if in_bounds:
            area = picture.crop(region)
            mask = Image.new('L', area.size, 255)
            if re.search(r'covering (?:their |the )?overlap', expect):
                # Only the explicitly described later overlay hides earlier pixels.
                for later, _, _ in regions[index+1:]:
                    left, top = max(x,later[0]), max(y,later[1])
                    r, b = min(right,later[2]), min(bottom,later[3])
                    if left < r and top < b:
                        ImageDraw.Draw(mask).rectangle((left-x,top-y,r-x-1,b-y-1), fill=0)
            visible = mask.histogram()[255]
            pixels = dict((c,n) for n,c in area.getcolors(area.width*area.height))
            difference = ImageChops.difference(area, Image.new('RGB',area.size,colour))
            channels = difference.split()
            mismatch = ImageChops.lighter(ImageChops.lighter(channels[0],channels[1]),channels[2])
            bad = ImageChops.multiply(mismatch.point(lambda v: 255 if v else 0),mask).histogram()[255]
            count = visible - bad
            measured.update(pixels=count, total=visible, occluded_pixels=area.width*area.height-visible,
                            share=count/visible if visible else None,
                            colours={colour_name(c):n for c,n in sorted(pixels.items())[:16]})
            ok = visible > 0 and bad == 0
        check('region', claim, dict(box=region, colour=colour), measured, ok)

    # Positive pass is deliberately a small, complete grammar. Everything else
    # retains its original prose as an unresolved claim alongside measured checks.
    simple_fill = filled and len(colours) == 1 and not points and re.fullmatch(
        r'(?:A|An) (?:entirely|solid|plain|uniform) [a-z -]+ \(#[0-9a-f]{6}\) '
        r'(?:\d+x\d+ (?:rectangle|square|field|patch)|full screen)'
        r'(?:; (?:no text or shapes|no shapes or text|no text|no marks or text))?\.', expect, re.I)
    simple_palette = exclusive and re.fullmatch(
        r'(?:A|An) \d+x\d+ picture containing only [a-z -]+ \(#[0-9a-f]{6}\)'
        r'(?: and [a-z -]+ \(#[0-9a-f]{6}\))+; nothing else\.', expect, re.I)
    if not simple_fill and not simple_palette:
        unresolved(expect, 'remaining shape, text, placement, count, edge or temporal prose requires independent review')
    if not checks:
        unresolved(expect, 'no mechanically checkable claims')
    result['verdict'] = 'fail' if result['failed_claims'] else 'unparseable' if unknown else 'pass'
    return result


def importer():
    spec = importlib.util.spec_from_file_location('screen_approve', Path(__file__).with_name('approve.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_set(manifest):
    approval = importer()
    doc = approval.read_json(manifest)
    approval.fields(doc, {'version', 'candidates_id', 'rows'})
    approval.require(type(doc['version']) is int and doc['version'] == 1, 'invalid candidates version')
    approval.require(approval.candidates_id(doc['rows']) == doc['candidates_id'], 'candidates id mismatch')
    report = dict(version=1, candidates_id=doc['candidates_id'],
                  manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(),
                  verifier_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  coordinates='zero-based top-left; rectangles half-open',
                  approval_eligible=False, rows=[])
    for index, row in enumerate(doc['rows']):
        identity = {k: row.get(k) for k in ('suite', 'case', 'checkpoint', 'mode', 'sha256',
                                          'png', 'expect', 'expectCommit', 'boot_id', 'commit')}
        try:
            approval.fields(row, approval.PICTURE | {'title', 'contract_section', 'binding', 'boot_id', 'commit', 'intent', 'png'},
                            {'expect', 'expectCommit'})
            approval.picture(row)
            approval.require(approval.matches(r'[a-zA-Z0-9][a-zA-Z0-9._-]*\.png', row['png']), 'invalid PNG path')
            path = manifest.parent / row['png']
            approval.require(path.resolve().parent == manifest.parent.resolve(), 'PNG escapes candidates directory')
            with Image.open(path) as image:
                approval.require(image.format == 'PNG' and image.mode == 'RGB' and
                                 image.size == tuple(map(int, row['mode'].split('x'))), 'PNG mode or dimensions mismatch')
                canonical = bytearray(image.tobytes('raw', 'BGRX'))
                canonical[3::4] = b'\xff' * (image.width*image.height)
                measured = hashlib.sha256(canonical).hexdigest()
                approval.require(measured == row['sha256'], f'PNG digest mismatch: measured {measured}')
                verdict = verify_picture(image, row.get('expect'), row['checkpoint'],
                                         case=row['case'], suite=row['suite'], commit=row['commit'])
        except (OSError, ValueError, KeyError, TypeError, Image.DecompressionBombError) as exc:
            failed = dict(kind='input', claim='verified candidate pixels', expected='valid candidate PNG',
                          measured=str(exc), verdict='fail')
            verdict = dict(verdict='fail', checked_claims=[failed], failed_claims=[failed], unparseable_claims=[])
        report['rows'].append(dict(index=index, **identity, **verdict))
    report['summary'] = {v: sum(r['verdict'] == v for r in report['rows']) for v in ('pass', 'fail', 'unparseable')}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('candidates', type=Path, help='candidates directory or manifest.json')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    try:
        manifest = args.candidates / 'manifest.json' if args.candidates.is_dir() else args.candidates
        report = verify_set(manifest)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        importer().atomic_write(args.output, (json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False)+'\n').encode())
        print(json.dumps(report['summary'], sort_keys=True))
        return 1 if report['summary']['fail'] else 2 if report['summary']['unparseable'] else 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f'verify_expect failed: {exc}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
