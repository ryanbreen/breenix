#!/usr/bin/env python3
"""Check Expect claims against candidate pixels without making approval decisions.

Coordinates are zero-based, top-left; (x,y)-(right,bottom) is half-open.
Unknown prose is reported, never treated as proof. Decimation cannot prove the
unsampled source pixels. Requires Pillow, as does approve.py.
"""
import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys

from PIL import Image, ImageChops, ImageDraw

HEX = re.compile(r'#[0-9A-Fa-f]{6}\b')
SIZE = re.compile(r'\b(\d+)x(\d+)\b')
POINT = re.compile(r'\((-?\d+),\s*(-?\d+)\)(?:\s*[-–]\s*\((-?\d+),\s*(-?\d+)\))?')
ROOT = Path(__file__).resolve().parents[2]


def rgb(value):
    return tuple(bytes.fromhex(value.lstrip('#')))


def colour_name(value):
    return '#%02X%02X%02X' % value


def layout(size, mode):
    w, h = size
    mw, mh = mode
    if min(w, h) <= 0 or max(w, h, mw, mh) > 16384:
        raise ValueError('dimensions outside intent layout')
    if size == mode:
        return (0, 0, w, h), 1, 1
    if min(mw, mh) <= 64:
        raise ValueError('intent mode too small for border')
    scale = min((mw - 64) // w, (mh - 64) // h)
    divisor = 1
    if scale == 0:
        scale = 1
        divisor = max((w + mw - 65) // (mw - 64), (h + mh - 65) // (mh - 64))
    width, height = (w + divisor - 1) // divisor * scale, (h + divisor - 1) // divisor * scale
    x, y = (mw - width) // 2, (mh - height) // 2
    return (x, y, x + width, y + height), scale, divisor


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


def verify_picture(image, expect, checkpoint='intent'):
    image = image.convert('RGB')
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
    end_size = re.search(r'end screen.*?across (\d+)x(\d+)', expect)
    judged_size = re.search(r'judged crop is (\d+)x(\d+)', expect)
    if checkpoint == 'end' and end_size:
        source = tuple(map(int, end_size.groups()))
    elif checkpoint == 'intent' and judged_size:
        source = tuple(map(int, judged_size.groups()))
    else:
        source = max((item[0] for item in dimensions), key=lambda s: s[0]*s[1], default=None)
    picture = image
    if checkpoint == 'intent' and source and source != image.size:
        try:
            box, scale, divisor = layout(source, image.size)
            template = checker(image.size)
            x, y, right, bottom = box
            ImageDraw.Draw(template).rectangle((x-2, y-2, right+1, bottom+1), fill='white')
            template.paste(image.crop(box), (x, y))
            mismatch = ImageChops.difference(image, template).getbbox()
            check('layout', 'intent checkerboard and 2 px white border', list(box),
                  dict(mismatch_box=list(mismatch) if mismatch else None), mismatch is None)
            crop = image.crop(box)
            if divisor == 1:
                picture = crop.resize(source, Image.Resampling.NEAREST)
                restored = picture.resize(crop.size, Image.Resampling.NEAREST)
                mismatch = ImageChops.difference(crop, restored).getbbox()
                check('scale', 'integer nearest-neighbour picture blocks', scale,
                      dict(mismatch_box=list(mismatch) if mismatch else None), mismatch is None)
            else:
                picture = crop
                unresolved('source pixels', f'intent decimation by {divisor} omits source pixels; '
                           'dimensions, colour shares and unsampled coordinates cannot be proved')
            result['picture'] = dict(size=list(picture.size), source_size=list(source),
                                     box=list(box), scale=scale, divisor=divisor,
                                     source_dimensions='Expect-derived; checked for layout compatibility')
        except ValueError as exc:
            check('layout', 'intent dimensions', list(source), str(exc), False)
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

    decimated = result['picture']['divisor'] > 1
    if source:
        if checkpoint == 'end' and source != image.size and re.search(r'(?:top left|top-left|screen origin).*charcoal|charcoal.*screen', expect, re.I):
            # The sentence explicitly describes a patch in a larger end frame.
            check('dimensions', 'top-left patch fits end frame', list(source), list(image.size),
                  source[0] <= image.width and source[1] <= image.height)
            unresolved('patch dimensions', 'end frame has no intent border; patch extent requires its shape or background claims')
        elif not decimated:
            check('dimensions', 'picture dimensions', list(source), list(picture.size), source == picture.size)
    elif not re.search(r'full screen|stage|screen', expect, re.I):
        unresolved('picture dimensions', 'no unambiguous picture dimensions in Expect')

    counts = Counter(dict((colour, count) for count, colour in picture.getcolors(picture.width * picture.height)))
    total = picture.width * picture.height
    colours = list(dict.fromkeys(m[0].upper() for m in HEX.finditer(expect)))
    for colour in colours:
        count = counts[rgb(colour)]
        if decimated and count == 0:
            unresolved(f'{colour} present', 'colour absent in decimated samples; unsampled source pixels unknown')
            continue
        check('colour', f'{colour} present', dict(minimum_pixels=1),
              dict(pixels=count, total=total, share=count/total), count > 0)

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
        if decimated:
            unresolved(point[0], 'decimated source coordinates cannot all be observed')
            continue
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
                verdict = verify_picture(image, row.get('expect'), row['checkpoint'])
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
