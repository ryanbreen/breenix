#!/usr/bin/env python3
"""Check ARM64 fixed reservations or report the physical layout of an LLD map."""
import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BLOCK = 0x200000


def match(pattern, text, label):
    found = re.search(pattern, text, re.MULTILINE)
    if not found:
        raise ValueError(f"cannot find {label}")
    return found


def number(value):
    value = value.replace('_', '')
    if value.endswith(('M', 'K')):
        return int(value[:-1], 0) * (1048576 if value[-1] == 'M' else 1024)
    return int(value, 0)


def rust_expr(text, name):
    return match(r'\bconst\s+' + name + r'\s*:\s*\w+\s*=\s*([^;]+);', text, name)[1].strip()


def layout(root):
    linker = (root / 'kernel/src/arch_impl/aarch64/linker.ld').read_text()
    loader = (root / 'parallels-loader/src/page_tables.rs').read_text()
    contract = (root / 'libs/arm64-boot-contract/src/lib.rs').read_text()
    constants = (root / 'kernel/src/arch_impl/aarch64/constants.rs').read_text()
    platform = (root / 'kernel/src/platform_config.rs').read_text()
    ceiling = match(r'ASSERT\s*\(\s*__kernel_writable_phys_end\s*<=\s*([^,\s]+)', linker, 'image ceiling')[1]
    if not re.fullmatch(r'[A-Za-z_]\w*', ceiling):
        raise ValueError('image ceiling must be a linker symbol shared with the loader stack bottom via the boot contract')
    boundary = number(match(r'^\s*' + re.escape(ceiling) + r'\s*=\s*(0x[\da-fA-F_]+)\s*;', linker, 'linker ceiling symbol')[1])
    loader_bottom = rust_expr(loader, 'BOOT_STACK_BOTTOM_PHYS')
    shared = match(r'^arm64_boot_contract::([A-Za-z_]\w*)$', loader_bottom, 'loader stack bottom contract reference')[1]
    if number(rust_expr(contract, shared)) != boundary:
        raise ValueError('linker ceiling differs from the shared loader stack bottom contract')
    top_ref = match(r'^arm64_boot_contract::([A-Za-z_]\w*)$', rust_expr(loader, 'BOOT_STACK_TOP_PHYS'), 'loader stack top contract reference')[1]
    top = number(rust_expr(contract, top_ref))
    base = number(match(r'^\s*KERNEL_PHYS_BASE\s*=\s*(0x[\da-fA-F_]+)', linker, 'image base')[1])
    stack_base = number(match(r'HHDM_BASE\s*\+\s*(0x[\da-fA-F_]+)', rust_expr(constants, 'PERCPU_STACK_REGION_BASE_DEFAULT'), 'per-CPU base')[1])
    stride = number(rust_expr(constants, 'PERCPU_STACK_STRIDE'))
    count = number(rust_expr(constants, 'MAX_CPUS'))
    dma = number(rust_expr(loader, 'NC_DMA_BASE'))
    dma_size_expr = rust_expr(loader, 'NC_DMA_SIZE')
    dma_size = number(rust_expr(loader, dma_size_expr)) if re.fullmatch(r'[A-Za-z_]\w*', dma_size_expr) else number(dma_size_expr)
    dma_link = number(match(r'\.dma\s*\(NOLOAD\)\s*:\s*AT\((0x[\da-fA-F_]+)\)', linker, 'linker DMA address')[1])
    if dma != dma_link:
        raise ValueError('linker and loader DMA addresses differ')
    section = match(r'const\s+KERNEL_FIXED_RANGES\s*:[^=]+?=\s*\[([\s\S]*?)\];', platform, 'fixed ranges')[1]
    ranges = []
    for start, end in re.findall(r'PhysRange::new\(\s*([^,]+),\s*([^\)]+)\)', section):
        def resolve(expr):
            expr = expr.strip()
            if expr.startswith('arm64_boot_contract::'):
                return number(rust_expr(contract, expr.split('::')[1]))
            return number(expr)
        ranges.append((resolve(start), resolve(end)))
    regions = [('image window', base, boundary), ('loader stack', boundary, top),
               ('per-CPU stacks', stack_base, stack_base + count * stride), ('DMA block', dma, dma + dma_size)]
    for name, start, end in regions:
        if start >= end or not any(lo <= start and end <= hi for lo, hi in ranges):
            raise ValueError(f'{name} is empty or leaves KERNEL_FIXED_RANGES')
    for i, (name, start, end) in enumerate(regions):
        for other, lo, hi in regions[i + 1:]:
            if start < hi and lo < end:
                raise ValueError(f'{name} overlaps {other}')
    check_image_literals(root, base, boundary)
    return base, boundary



def check_image_literals(root, base, boundary):
    """Refuse hard-coded image addresses in ARM Rust source.

    The legacy MMU's kernel/user permission boundary is a range endpoint,
    not a location of an object or page table; it is the sole exception.
    Comments and string literals describe addresses rather than use them.
    """
    hits = []
    for path in sorted((root / 'kernel/src/arch_impl/aarch64').glob('*.rs')):
        for line_no, line in enumerate(path.read_text().splitlines(), 1):
            code = re.sub(r'"(?:\\.|[^"\\])*"', '""', line.split('//', 1)[0])
            for literal in re.finditer(r'\b0x[0-9a-fA-F_]+', code):
                value = number(literal.group())
                if base <= value < boundary:
                    if path.name == 'mmu.rs' and re.fullmatch(
                            r'\s*const KERNEL_REGION_END: u64 = 0x4100_0000;\s*', code):
                        continue
                    hits.append(f'{path.relative_to(root)}:{line_no}: {literal.group()}')
    if hits:
        raise ValueError('hex literals inside image window:\n' + '\n'.join(hits))

def map_report(text, ceiling):
    # LLD map columns: VMA, LMA, size, alignment, output section.  Only
    # unindented output-section rows count; input subsections are ignored.
    sections = {}
    for line in text.splitlines():
        m = re.match(r'^\s*([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+\d+\s+(\.[\w.]+)\s*$', line)
        if m:
            _, lma, size, name = m.groups()
            sections[name] = (int(lma, 16), int(lma, 16) + int(size, 16))
    try:
        text_span = (sections['.text.vectors'][0], sections['.text'][1])
        ro = sections['.rodata']
        writable = (sections['.data'][0], sections['.bss.stack'][1])
    except KeyError as e:
        raise ValueError(f'map missing output section {e.args[0]}') from e
    if not (text_span[0] <= text_span[1] <= ro[0] <= ro[1] <= writable[0] <= writable[1]):
        raise ValueError('map sections overlap or are out of order')
    if ro[0] % BLOCK or writable[0] % BLOCK:
        raise ValueError('map W^X boundaries are not 2 MiB aligned')
    for name, (start, end) in [('text', text_span), ('rodata', ro), ('writable', writable)]:
        print(f'{name}: {start:#x}..{end:#x} ({end-start:#x} bytes)')
    print(f'2 MiB W^X padding: text={ro[0]-text_span[1]:#x} rodata={writable[0]-ro[1]:#x}')
    margin = ceiling - writable[1]
    print(f'margin: {margin:+#x} bytes (ceiling {ceiling:#x})')
    if margin < 0:
        raise ValueError('writable image exceeds image ceiling')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--map', type=Path, help='LLD -Map file; report against current linker ceiling')
    args = parser.parse_args()
    try:
        if args.map:
            linker = (args.root / 'kernel/src/arch_impl/aarch64/linker.ld').read_text()
            ceiling = match(r'ASSERT\s*\(\s*__kernel_writable_phys_end\s*<=\s*([^,\s]+)', linker, 'image ceiling')[1]
            if re.fullmatch(r'[A-Za-z_]\w*', ceiling):
                ceiling = match(r'^\s*' + re.escape(ceiling) + r'\s*=\s*(0x[\da-fA-F_]+)', linker, 'ceiling symbol')[1]
            map_report(args.map.read_text(), number(ceiling))
        else:
            layout(args.root)
            print('ARM64 fixed image layout: OK')
    except (ValueError, OSError) as e:
        print(f'ARM64 image layout: {e}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
