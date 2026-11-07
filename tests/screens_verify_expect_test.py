"""Pixel checks use synthetic pictures, independent of retained gallery evidence."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/screens/verify_expect.py'


def load_verifier():
    spec = importlib.util.spec_from_file_location('verify_expect', SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def intent(picture, mode=(160, 128)):
    """Independent renderer for the specified enlargement layout."""
    frame = Image.new('RGB', mode)
    draw = ImageDraw.Draw(frame)
    for y in range(0, mode[1], 16):
        for x in range(0, mode[0], 16):
            c = '#7A7A7A' if (x // 16 + y // 16) % 2 else '#5A5A5A'
            draw.rectangle((x, y, x + 15, y + 15), fill=c)
    scale = min((mode[0] - 64) // picture.width, (mode[1] - 64) // picture.height)
    enlarged = picture.resize((picture.width * scale, picture.height * scale), Image.Resampling.NEAREST)
    x, y = (mode[0] - enlarged.width) // 2, (mode[1] - enlarged.height) // 2
    draw.rectangle((x-2, y-2, x+enlarged.width+1, y+enlarged.height+1), fill='white')
    frame.paste(enlarged, (x, y))
    return frame


class VerifyExpectTest(unittest.TestCase):
    def setUp(self):
        self.verify = load_verifier().verify_picture

    def test_claim_parser_distinguishes_full_end_screen_from_top_left_strip(self):
        image = Image.new('RGB', (64, 32), '#181512')
        image.paste(Image.new('RGB', (20, 1), '#800000'), (0, 0))
        expect = ('The end screen fills 64x32 with charcoal (#181512); '
                  'top left is a 20x1 strip in dark red (#800000); no text.')
        self.assertFalse(self.verify(image, expect, 'end')['failed_claims'])
        self.assertTrue(self.verify(image, expect.replace('64x32', '65x32'), 'end')['failed_claims'])

    def test_claim_parser_checks_lcd_blend_output_instead_of_endpoint_presence(self):
        image = Image.new('RGB', (256, 4))
        back, front = (0x31, 0x27, 0x19), (0xb7, 0x63, 0x21)
        for x in range(256):
            alpha = ((37*x)%256, 255-x, x)
            pixel = tuple((front[c]*alpha[c]+back[c]*(255-alpha[c])+127)//255 for c in range(3))
            for y in range(4): image.putpixel((x, y), pixel)
        expect = ('A 256x4 picture; B,G,R coverage is x, 255-x and (37*x modulo 256), '
                  'blended between dark brown (#312719) and rust (#B76321); rows are identical.')
        for checkpoint, frame, sentence in (
                ('intent', image.copy(), expect),
                ('end', Image.new('RGB', (320, 32), '#181512'),
                 'The end screen fills 320x32 with charcoal (#181512); top left is a 256x4 picture; '+expect.split('; ', 1)[1])):
            if checkpoint == 'end': frame.paste(image, (0, 0))
            self.assertFalse(self.verify(frame, sentence, checkpoint)['failed_claims'])
            frame.putpixel((123, 2), (1, 2, 3))
            self.assertTrue(self.verify(frame, sentence, checkpoint)['failed_claims'])
        extra = self.verify(image, expect+' #312719 is present.', 'intent')['failed_claims']
        self.assertTrue(any(c['kind'] == 'colour' and c['claim'] == '#312719 present' for c in extra))

    def test_crop_and_entire_colour_share(self):
        picture = Image.new('RGB', (8, 6), '#123456')
        frame = intent(picture)
        result = self.verify(frame, 'An entirely blue (#123456) 8x6 rectangle; no text or shapes.')
        self.assertEqual(result['verdict'], 'pass', result)
        self.assertEqual(result['picture']['size'], [8, 6])
        share = next(c for c in result['checked_claims'] if c['kind'] == 'palette')
        self.assertEqual(share['measured']['share'], 1.0)
        picture.putpixel((2, 3), (255, 0, 0))
        result = self.verify(intent(picture), 'An entirely blue (#123456) 8x6 rectangle; no text or shapes.')
        self.assertEqual(result['verdict'], 'fail', result)
        self.assertTrue(result['failed_claims'])

    def test_commit_mosaic_checks_formula_and_conditional_example(self):
        expect = ('A 256x128 mosaic picture with blue column ramps, green row ramps '
                  'and red diagonal flecks; seed k is the captured nonzero commit id '
                  '(for k=1 the upper-left pixel is dark green (#011D0D)), '
                  'with B=(7*x+13*k)%256, G=(5*y+29*k)%256, '
                  'R=(x XOR y XOR k)%256; no text.')
        for seed in (1, 200, 256):
            picture = Image.new('RGB', (256, 128))
            picture.putdata([((x ^ y ^ seed) % 256, (5*y+29*seed) % 256,
                              (7*x+13*seed) % 256)
                             for y in range(128) for x in range(256)])
            result = self.verify(picture, expect, 'intent')
            self.assertFalse(result['failed_claims'], result)
            formula = next(c for c in result['checked_claims'] if c['kind'] == 'mosaic')
            self.assertEqual(formula['measured']['compared_pixels'], 32768)
            picture.putpixel((123, 45), (1, 2, 3))
            self.assertTrue(self.verify(picture, expect, 'intent')['failed_claims'])
        self.assertTrue(self.verify(picture, expect.replace('#011D0D', '#011D0E'), 'intent')['failed_claims'])

    def test_bad_dimensions_border_background_and_scaled_pixels(self):
        frame = intent(Image.new('RGB', (8, 6), '#123456'))
        for damaged in ('dimensions', 'border', 'background', 'scaled'):
            image = frame.copy()
            expect = 'An entirely blue (#123456) 8x6 rectangle.'
            if damaged == 'dimensions':
                expect = 'An entirely blue (#123456) 9x6 rectangle.'
            elif damaged == 'border':
                image.putpixel((47, 39), (0, 0, 0))
            elif damaged == 'background':
                image.putpixel((0, 0), (0, 0, 0))
            else:
                image.putpixel((49, 41), (1, 2, 3))
            self.assertEqual(self.verify(image, expect)['verdict'], 'fail', damaged)

    def test_mode_and_end_are_not_cropped(self):
        image = Image.new('RGB', (8, 6), '#123456')
        for checkpoint in ('end', 'intent'):
            result = self.verify(image, 'A solid blue (#123456) 8x6 rectangle.', checkpoint)
            self.assertEqual(result['verdict'], 'pass', result)
            self.assertEqual(result['picture']['box'], [0, 0, 8, 6])
        result = self.verify(image, 'An entirely blue (#123456) 9x6 rectangle.', 'end')
        self.assertEqual(result['verdict'], 'fail', result)

    def test_every_named_colour_and_exclusive_palette(self):
        image = Image.new('RGB', (8, 6), '#000000')
        image.putpixel((2, 3), (255, 0, 0))
        result = self.verify(image, 'An 8x6 picture containing only black (#000000) and red (#FF0000); nothing else.', 'end')
        self.assertEqual(result['verdict'], 'pass', result)
        image.putpixel((0, 0), (0, 0, 255))
        result = self.verify(image, 'An 8x6 picture containing only black (#000000) and red (#FF0000); nothing else.', 'end')
        self.assertEqual(result['verdict'], 'fail', result)
        result = self.verify(image, 'An 8x6 picture with green (#00FF00).', 'end')
        self.assertTrue(any(c['kind'] == 'colour' for c in result['failed_claims']))

    def test_points_rectangles_and_size_at_origin(self):
        image = Image.new('RGB', (8, 6), '#000000')
        ImageDraw.Draw(image).rectangle((1, 1, 3, 2), fill='#FF0000')
        image.putpixel((7, 5), (0, 255, 0))
        for sentence in (
            'An 8x6 picture with red (#FF0000) at (1,1)-(4,3) and green (#00FF00) at (7,5).',
            'An 8x6 picture with a red (#FF0000) 3x2 rectangle at (1,1) and one green (#00FF00) pixel at (7,5).',
        ):
            result = self.verify(image, sentence, 'end')
            self.assertFalse(result['failed_claims'], result)
            self.assertEqual(len([c for c in result['checked_claims'] if c['kind'] == 'region']), 2)
            image.putpixel((2, 2), (0, 0, 0))
            self.assertEqual(self.verify(image, sentence, 'end')['verdict'], 'fail')
            image.putpixel((2, 2), (255, 0, 0))
        result = self.verify(image, 'An 8x6 picture with green (#00FF00) at (8,6).', 'end')
        self.assertEqual(result['verdict'], 'fail', result)

    def test_unknown_claims_never_silently_pass(self):
        image = Image.new('RGB', (8, 6), '#000000')
        for sentence in ('An 8x6 black (#000000) picture with a beautiful spiral.',
                         'Black (#000000) text at (2,3) in an 8x6 picture.',
                         'An 8x6 black (#000000) picture, with a red rectangle at (1,1)-(3,3).',
                         'An 8x6 picture with no text.', ''):
            result = self.verify(image, sentence, 'end')
            self.assertEqual(result['verdict'], 'unparseable', result)
            self.assertTrue(result['unparseable_claims'])

    def test_fail_takes_precedence_over_unparseable(self):
        image = Image.new('RGB', (8, 6), '#000000')
        result = self.verify(image, 'An 8x6 picture with red (#FF0000) and a beautiful spiral.', 'end')
        self.assertEqual(result['verdict'], 'fail', result)
        self.assertTrue(result['unparseable_claims'])

    def test_legacy_decimation_is_refused(self):
        # A 320x8 source decimates by 4 into an 80x2 picture, centred with a border.
        frame = Image.new('RGB', (160, 128), '#5A5A5A')
        draw = ImageDraw.Draw(frame)
        for y in range(0, 128, 16):
            for x in range(0, 160, 16):
                if (x//16+y//16) % 2:
                    draw.rectangle((x,y,x+15,y+15), fill='#7A7A7A')
        draw.rectangle((38,61,121,66), fill='white')
        draw.rectangle((40,63,119,64), fill='#123456')
        result = self.verify(frame, 'An entirely blue (#123456) 320x8 rectangle.')
        self.assertEqual(result['verdict'], 'fail', result)
        self.assertTrue(any(c['kind']=='layout' for c in result['failed_claims']))

    def test_strip_folds_keep_both_rows_and_markers(self):
        v = load_verifier()
        picture = Image.new('RGB',(16384,2),'#D15311')
        ImageDraw.Draw(picture).line((0,0,16383,0),fill='#3D07F1')
        for x,c in [(0,'black'),(8191,'#345678'),(8192,'#345678'),(16383,'white')]:
            picture.putpixel((x,1), Image.new('RGB',(1,1),c).getpixel((0,0)))
        frame = v.checker((2560,1600))
        draw=ImageDraw.Draw(frame)
        draw.rectangle((0,791,2559,808),fill='white')
        # Seven 2556-wide source bands, each retaining the two source rows.
        for i,x in enumerate(range(0,16384,2556)):
            frame.paste(picture.crop((x,0,min(x+2556,16384),2)),(2,793+2*i))
        restored,page,n,bad,scale_bad = v.restore_intent(frame,picture.size,'intent')
        self.assertEqual(n,1)
        self.assertIsNone(bad)
        self.assertIsNone(scale_bad)
        self.assertEqual(restored.tobytes(),picture.tobytes())
        frame.putpixel((2,794),(1,2,3))
        self.assertNotEqual(v.restore_intent(frame,picture.size,'intent')[0].tobytes(),picture.tobytes())

    def test_paged_panel_union_and_tile_scale(self):
        v=load_verifier()
        for case in ('png-decode/interlace-small','png-decode/ancillary-ignored','png-decode/independent-fixtures','png-encode/sizes'):
            row=next(r for r in v.intent_manifest.inventory() if r['suite']=='breengel' and r['case']==case)
            # The source encodes coordinates, including label and padding pixels.
            w,h=row['size']
            source=Image.frombytes('RGB',(w,h),bytes(c for y in range(h) for x in range(w) for c in (x%251,y%251,(x+y)%251)))
            seen=bytearray(w*h)
            pages=v.intent_manifest.row_plans(row)
            self.assertEqual(len(pages),2 if case in ('png-decode/interlace-small','png-decode/ancillary-ignored') else 1)
            for i,p in enumerate(pages):
                self.assertEqual(p['scale'],1)
                frame=v.checker(row['mode'])
                x,y,r,b=p['border']
                self.assertTrue(0<=x<r<=2560 and 0<=y<b<=1600)
                ImageDraw.Draw(frame).rectangle((x,y,r-1,b-1),fill='white')
                for (dx,dy,dw,dh),(sx,sy,sw,sh) in p['tiles']:
                    self.assertEqual((dw,dh),(sw,sh))
                    frame.paste(source.crop((sx,sy,sx+sw,sy+sh)),(dx,dy))
                    for line in range(sy,sy+sh):
                        for column in range(sx,sx+sw):
                            seen[line*w+column]+=1
                checkpoint='intent' if len(pages)==1 else f'intent-p{i+1}'
                restored,_,_,bad,scale_bad=v.restore_intent(frame,(w,h),checkpoint,v.intent_manifest.cells(row))
                self.assertIsNone(bad)
                self.assertIsNone(scale_bad)
                for _,(sx,sy,sw,sh) in p['tiles']:
                    box=(sx,sy,sx+sw,sy+sh)
                    self.assertEqual(restored.crop(box).tobytes(),source.crop(box).tobytes())
            self.assertEqual(set(seen),{1})
            if len(pages)>1:
                with self.assertRaises(ValueError):
                    v.restore_intent(frame,(w,h),'intent',v.intent_manifest.cells(row))

    def test_uniform_prose_does_not_hide_additional_claims(self):
        image = Image.new('RGB', (8, 6), '#000000')
        for sentence in ('A solid black (#000000) 8x6 rectangle with a dragon.',
                         'A solid black (#000000) 8x6 rectangle, restored after an earlier frame.'):
            result = self.verify(image, sentence, 'end')
            self.assertEqual(result['verdict'], 'unparseable', result)

    def test_overlapping_rectangles_check_visible_pixels(self):
        image = Image.new('RGB', (8, 6), '#404040')
        draw = ImageDraw.Draw(image)
        draw.rectangle((1,1,2,2), fill='#FF0000')
        draw.rectangle((2,0,4,1), fill='#0000FF')
        sentence = ('An 8x6 grey (#404040) patch with red (#FF0000) at (1,1)-(3,3) '
                    'and blue (#0000FF) at (2,0)-(5,2), blue covering the overlap; no text.')
        result = self.verify(image, sentence, 'end')
        self.assertFalse(result['failed_claims'], result)
        self.assertEqual(len([c for c in result['checked_claims'] if c['kind'] == 'region']), 2)
        image.putpixel((1,2), (0,0,0))
        self.assertEqual(self.verify(image, sentence, 'end')['verdict'], 'fail')

    def test_size_after_coordinate_checks_the_whole_rectangle(self):
        image = Image.new('RGB', (32,24), '#0A141E')
        ImageDraw.Draw(image).rectangle((10,2,21,6), fill='#FA0000')
        sentence = 'On navy (#0A141E), red (#FA0000) at (10,2), size 12x5, in a 32x24 picture.'
        self.assertFalse(self.verify(image, sentence, 'end')['failed_claims'])
        image.putpixel((21,6), (0,0,0))
        self.assertEqual(self.verify(image, sentence, 'end')['verdict'], 'fail')

    def test_end_frame_uses_its_explicit_dimensions(self):
        image = Image.new('RGB', (160,128), '#0000FF')
        ImageDraw.Draw(image).rectangle((3,3,8,8), fill='#FF0000')
        sentence = ('A red (#FF0000) 6x6 square at (3,3) on blue (#0000FF); '
                    'the judged crop is 16x12, while the end screen extends the same blue field '
                    'across 160x128, with no text.')
        result = self.verify(image, sentence, 'end')
        self.assertFalse(result['failed_claims'], result)

    def test_interior_strips_and_grid_counts_are_not_picture_dimensions(self):
        image = Image.new('RGB', (160,128), '#0C131F')
        ImageDraw.Draw(image).rectangle((40,40,100,80), fill='white')
        for sentence in (
            'On navy (#0C131F), white (#FFFFFF) labels head sample rows. A 3x1 strip is white.',
            'Sixteen 8x5 mosaic rectangles form a 4x4 grid on navy (#0C131F); white (#FFFFFF) labels.',
        ):
            result = self.verify(image, sentence)
            self.assertFalse(result['failed_claims'], result)
            self.assertEqual(result['picture']['box'], [0,0,160,128])
            self.assertEqual(result['verdict'], 'unparseable')

    def test_unknown_source_size_still_crops_only_a_verified_border(self):
        image = intent(Image.new('RGB',(8,6),'#123456'))
        result = self.verify(image, 'A blue (#123456) picture with a white (#FFFFFF) mark.')
        self.assertNotEqual(result['picture']['box'], [0,0,160,128])
        image.putpixel((0,0),(0,0,0))
        result = self.verify(image, 'A blue (#123456) picture with a white (#FFFFFF) mark.')
        self.assertEqual(result['picture']['box'], [0,0,160,128])
        self.assertEqual(result['verdict'], 'unparseable')

    def test_text_colour_is_near_anchor_not_at_origin(self):
        sentence = ("A 96x48 picture on black (#000000) background; the text 'Wi é A', "
                    "white (#FFFFFF), starting near x=8, y=12, font size 16.")
        image = Image.new('RGB', (96,48), 'black')
        # Glyph ink need not touch the origin. No font renderer is under test here.
        image.putpixel((13,18), (255,255,255))
        result = self.verify(image, sentence, 'end')
        text = [c for c in result['checked_claims'] if c['kind'] == 'text-colour']
        self.assertEqual(len(text), 1, result)
        self.assertEqual(text[0]['verdict'], 'pass')
        self.assertEqual(result['verdict'], 'unparseable')  # no OCR proof of the string
        image.putpixel((13,18), (0,0,0))
        image.putpixel((95,47), (255,255,255))
        result = self.verify(image, sentence, 'end')
        self.assertTrue(any(c['kind'] == 'text-colour' for c in result['failed_claims']), result)

    def test_text_background_must_be_majority_inside_picture(self):
        sentence = ("A 96x48 picture on black (#000000) background; the text 'A', "
                    "white (#FFFFFF), starting near x=8, y=12, font size 16.")
        image = Image.new('RGB', (96,48), '#000080')
        image.putpixel((0,0), (0,0,0))
        image.putpixel((13,18), (255,255,255))
        result = self.verify(intent(image, (256,192)), sentence)
        self.assertTrue(any(c['kind'] == 'background' for c in result['failed_claims']), result)
        image = Image.new('RGB', (96,48), 'black')
        image.putpixel((13,18), (255,255,255))
        result = self.verify(intent(image, (256,192)), sentence)
        background = next(c for c in result['checked_claims'] if c['kind'] == 'background')
        self.assertEqual(background['verdict'], 'pass')
        self.assertEqual(background['measured']['total'], 96*48)

    def test_cli_reports_every_row_and_refuses_tampering(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            image = Image.new('RGB', (8, 6), '#000000')
            image.save(path / 'one.png')
            row = dict(suite='gpu', case='draw/one', checkpoint='end', mode='8x6', binding='8x6',
                       sha256=hashlib.sha256(bytes((0,0,0,255))*48).hexdigest(), png='one.png',
                       title='One', intent='One', contract_section='example', boot_id='run', commit='a'*40,
                       expect='An entirely black (#000000) 8x6 rectangle.', expectCommit='b'*40)
            evidence = {k:v for k,v in row.items() if k not in ('expect','expectCommit')}
            cid = hashlib.sha256(json.dumps(dict(version=1, rows=[evidence]), sort_keys=True,
                    separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
            manifest = path / 'manifest.json'
            manifest.write_text(json.dumps(dict(version=1, candidates_id=cid, rows=[row])))
            report = path / 'report.json'
            command = [sys.executable, str(SCRIPT), str(path), '--output', str(report)]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            doc = json.loads(report.read_text())
            self.assertEqual(doc['candidates_id'], cid)
            self.assertEqual(len(doc['rows']), 1)
            self.assertEqual(doc['rows'][0]['verdict'], 'pass')
            image.putpixel((0,0), (1,2,3)); image.save(path / 'one.png')
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(json.loads(report.read_text())['rows'][0]['verdict'], 'fail')


if __name__ == '__main__':
    unittest.main()
