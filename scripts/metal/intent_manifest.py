"""U11 intent inventory and lossless screen layout, shared by host consumers."""
from functools import lru_cache
from pathlib import Path


@lru_cache(maxsize=1)
def inventory():
    rows = []
    for line in Path(__file__).with_name('intent-pictures.tsv').read_text().splitlines():
        if line.startswith('#') or not line:
            continue
        index, suite, case, w, h, mw, mh, xs, ys = line.split('\t')
        cuts = lambda s: [] if s == '-' else list(map(int, s.split(',')))
        rows.append(dict(index=int(index), suite=suite, case=case, size=(int(w), int(h)),
                         mode=(int(mw), int(mh)), xs=cuts(xs), ys=cuts(ys)))
    return rows


def cells(row):
    return [(x, y, r-x, b-y) for y,b in zip(row['ys'], row['ys'][1:])
            for x,r in zip(row['xs'], row['xs'][1:])]


def plans(size, mode, source_cells=()):
    """Return pages containing (destination, source) rectangles; sizes are widths/heights."""
    w,h = size
    mw,mh = mode
    if min(w,h) <= 0 or min(mw,mh) <= 64 or max(w,h,mw,mh) > 16384:
        raise ValueError('dimensions outside intent layout')
    def center(width, height, scale, tiles, border=True):
        x,y = (mw-width)//2, (mh-height)//2
        return dict(box=(x,y,x+width,y+height), scale=scale,
                    border=(x-2,y-2,x+width+2,y+height+2) if border else None,
                    tiles=[((dx+x,dy+y,dw,dh),src) for (dx,dy,dw,dh),src in tiles])
    if tuple(size) == tuple(mode):
        return [center(w,h,1,[((0,0,w,h),(0,0,w,h))],False)]
    scale = min((mw-64)//w,(mh-64)//h)
    if scale or (w <= mw-4 and h <= mh-4):
        scale = max(scale,1)
        return [center(w*scale,h*scale,scale,[((0,0,w*scale,h*scale),(0,0,w,h))])]
    bw,bh = mw-4,mh-4
    source_cells = list(source_cells)
    if not source_cells:
        if w <= bw and w*h > bw*bh:
            source_cells = [(0,y,w,min(bh,h-y)) for y in range(0,h,bh)]
        else:
            cw,ch = (bw,h) if h <= bh and w > bw else (min(32,w),min(32,h))
            source_cells = [(x,y,min(cw,w-x),min(ch,h-y)) for y in range(0,h,ch) for x in range(0,w,cw)]
    result, tiles = [], []
    x=y=row_h=width=0
    for src in source_cells:
        sx,sy,sw,sh = src
        if min(sw,sh) <= 0 or sx+sw > w or sy+sh > h or sw > bw or sh > bh:
            raise ValueError('intent cell outside source or page')
        if x+sw > bw:
            x,y,row_h = 0,y+row_h,0
        if y+sh > bh:
            result.append(center(width,y+row_h,1,tiles))
            tiles=[]
            x=y=row_h=width=0
        tiles.append(((x,y,sw,sh),src))
        x += sw
        width,row_h = max(width,x),max(row_h,sh)
    if tiles:
        result.append(center(width,y+row_h,1,tiles))
    return result


def row_plans(row, mode=None):
    return plans(row['size'], mode or row['mode'], cells(row))


def counts():
    result = {}
    for row in inventory():
        result[row['suite']] = result.get(row['suite'],0)+len(row_plans(row))
    return result


def checkpoint_names(suite, case):
    row=next((r for r in inventory() if r['suite']==suite and r['case']==case),None)
    count=len(row_plans(row)) if row else 1
    return ['intent'] if count==1 else [f'intent-p{i+1}' for i in range(count)]
