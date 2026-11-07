"""Independent synthetic panels exercise the checks before retained evidence."""
import importlib.util
from pathlib import Path
import unittest

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verify_expect', ROOT / 'scripts/screens/verify_expect.py')
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)
EXPECT = ('On navy (#0C131F), white-titled tiles have actual, reference, difference rows: '
          'scaled colour and grey alpha pairs with white outlines; differences black. '
          'A 3x1 strip is light grey (#C8C8C8), muted red (#A46464), light grey; '
          'alpha is white (#FFFFFF).')


def panel():
    # images.rs source-over: 3x1, scale 85, two 255x85 pictures per row.
    # Layout is specified here independently, without calling verifier helpers.
    image = Image.new('RGB', (538, 361), '#0C131F')
    draw = ImageDraw.Draw(image)
    for row in range(3):
        y = 36 + row * 111
        for column in range(2):
            x = 8 + column * 265
            draw.rectangle((x, y, x+256, y+86), fill='white')
            values = [(200,200,200), (164,100,100), (200,200,200)]
            if column == 1:
                values = [(255,255,255)] * 3
            if row == 2:
                values = [(0,0,0)] * 3
            for i, colour in enumerate(values):
                draw.rectangle((x+1+i*85, y+1, x+(i+1)*85, y+85), fill=colour)
    return image


class VerifyPanelTest(unittest.TestCase):
    def verify(self, image, expect=EXPECT):
        return verifier.verify_picture(image, expect, 'intent', case='png-decode/blit', suite='breengel')

    def test_pairs_differences_and_literal_bgra(self):
        result = self.verify(panel())
        self.assertFalse(result['failed_claims'], result)
        kinds = {c['kind'] for c in result['checked_claims']}
        self.assertTrue({'panel-pair', 'panel-difference', 'panel-reference', 'panel-scale'} <= kinds)
        # Text claims remain unresolved; mechanical evidence is not approval.
        self.assertFalse(result.get('approval_eligible', False))
        self.assertTrue(result['unparseable_claims'])
        for damage in ('pair', 'alpha-pair', 'difference', 'alpha-difference', 'literal', 'scale', 'border'):
            image = panel()
            draw = ImageDraw.Draw(image)
            if damage == 'pair':
                draw.rectangle((94,37,178,121), fill='red')
            elif damage == 'alpha-pair':
                draw.rectangle((274,37,358,121), fill='#808080')
            elif damage == 'difference':
                image.putpixel((9,259), (1,0,0))
            elif damage == 'alpha-difference':
                image.putpixel((274,259), (1,1,1))
            elif damage == 'literal':
                # Equal actual/reference plus a black difference must NOT suffice.
                for y in (37,148):
                    draw.rectangle((94,y,178,y+84), fill='#6464A4')
            elif damage == 'scale':
                image.putpixel((10,38), (1,2,3))
            else:
                image.putpixel((8,36), (0,0,0))
            result = self.verify(image)
            self.assertEqual(result['verdict'], 'fail', (damage, result))

    def test_intent_wrapper_and_mode_sized_panel(self):
        picture = panel()
        frame = Image.new('RGB', (1280,800))
        draw = ImageDraw.Draw(frame)
        for y in range(0,800,16):
            for x in range(0,1280,16):
                draw.rectangle((x,y,x+15,y+15), fill='#7A7A7A' if (x//16+y//16)%2 else '#5A5A5A')
        draw.rectangle((100,37,1179,762), fill='white')
        frame.paste(picture.resize((1076,722), Image.Resampling.NEAREST), (102,39))
        result = self.verify(frame)
        self.assertFalse(result['failed_claims'], result)
        self.assertEqual(result['picture']['scale'], 2)
        frame.putpixel((0,0), (0,0,0))
        self.assertEqual(self.verify(frame)['verdict'], 'fail')
        self.assertEqual(self.verify(picture)['picture']['box'], [0,0,538,361])

    def test_folded_pngsuite_checks_visible_fixture_pixels(self):
        case='png-decode/independent-fixtures'
        fixtures,_=verifier.panel_fixtures(case)
        size,tiles,_,_=verifier.panel_plan(fixtures)
        source=Image.new('RGB',size,'#0C131F')
        draw=ImageDraw.Draw(source)
        for fixture,tile in zip(fixtures,tiles):
            raw=fixture['pixels']
            colour=Image.frombytes('RGB',(fixture['width'],fixture['height']),bytes(c for i in range(0,len(raw),4) for c in (raw[i+2],raw[i+1],raw[i])))
            alpha=Image.frombytes('RGB',colour.size,bytes(c for i in range(3,len(raw),4) for c in (raw[i],)*3))
            for row in range(3):
                for column,plane in enumerate((colour,alpha)):
                    x=tile['x']+8+column*(tile['picture_w']+10)
                    y=tile['y']+8+tile['title_height']+row*(26+tile['picture_h'])+16
                    w,h=tile['picture_w'],tile['picture_h']
                    draw.rectangle((x,y,x+w+1,y+h+1),fill='white')
                    pixels=plane.resize((w,h),Image.Resampling.NEAREST) if row<2 else Image.new('RGB',(w,h),'black')
                    source.paste(pixels,(x+1,y+1))
        inventory=next(r for r in verifier.intent_manifest.inventory() if r['case']==case)
        page=verifier.intent_manifest.row_plans(inventory)[0]
        frame=verifier.checker((2560,1600))
        x,y,r,b=page['border']
        ImageDraw.Draw(frame).rectangle((x,y,r-1,b-1),fill='white')
        ImageDraw.Draw(frame).rectangle((x+2,y+2,r-3,b-3),fill='#0C131F')
        for (dx,dy,dw,dh),(sx,sy,sw,sh) in page['tiles']:
            frame.paste(source.crop((sx,sy,sx+sw,sy+sh)),(dx,dy))
        result=verifier.verify_picture(frame,'PngSuite fixture comparison','intent',case=case,suite='breengel')
        self.assertFalse(result['failed_claims'],result)
        self.assertEqual(len(result['panel']['tiles']),72)
        self.assertEqual(result['picture']['scale'],1)
        # U15: the empty final band is navy, not leftover white border fill.
        tail=frame.copy()
        tail.putpixel((1280,1247),(255,255,255))
        result=verifier.verify_picture(tail,'PngSuite fixture comparison','intent',case=case,suite='breengel')
        self.assertTrue(result['failed_claims'],result)
        # First fixture's actual colour plane: one altered visible pixel must fail.
        dx,dy,_,_=page['tiles'][0][0]
        frame.putpixel((dx+9,dy+37),(1,2,3))
        result=verifier.verify_picture(frame,'PngSuite fixture comparison','intent',case=case,suite='breengel')
        self.assertEqual(result['verdict'],'fail',result)

    def test_missing_expect_and_unknown_panel_never_pass(self):
        self.assertNotEqual(self.verify(panel(), '')['verdict'], 'pass')
        result = verifier.verify_picture(panel(), EXPECT, 'intent', case='png-decode/new-case', suite='breengel')
        self.assertNotEqual(result['verdict'], 'pass')


if __name__ == '__main__':
    unittest.main()
