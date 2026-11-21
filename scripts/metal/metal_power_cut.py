"""Command-line options shared by the host launcher and leased boot driver."""
import argparse
import math


FEATURES = ('breenfs_commit_window', 'breenfs_corrupt_inject')


def positive_seconds(value):
    seconds = float(value)
    if not math.isfinite(seconds) or seconds <= 0:
        raise argparse.ArgumentTypeError('expected finite positive seconds')
    return seconds


def add_options(parser):
    cut = parser.add_mutually_exclusive_group()
    cut.add_argument('--power-cut-after', type=positive_seconds, metavar='S',
                     help='force the chassis off S seconds after power-on on a BreenFS test-root suite boot')
    cut.add_argument('--power-cut-window', choices=('node-writes', 'first-flush', 'between-rings'),
                     help='force the chassis off at this BreenFS commit marker during the suite')
    cut.add_argument('--breenfs-bit-flip', action='store_true',
                     help='flip and verify one data bit on the disposable BreenFS writer root')


def options(args):
    after = getattr(args, 'power_cut_after', None)
    window = getattr(args, 'power_cut_window', None)
    if after is None and window is None:
        return []
    return ['--power-cut-after', str(after)] if window is None else ['--power-cut-window', window]


def kernel_fault(args):
    if window := getattr(args, 'power_cut_window', None):
        return FEATURES[0], f'breenfs_commit_window={window} breenfs_after_receipt=1'
    if (getattr(args, 'power_cut_after', None) is not None
            and getattr(args, 'case', None) == 'power-cut/writer'):
        # Timed runs use the same fault build with no guest publication hold.
        return FEATURES[0], None
    if getattr(args, 'breenfs_bit_flip', False):
        return FEATURES[1], 'breenfs_corrupt_inject=first-data:64:0'
    return None


def validate(args):
    fault = kernel_fault(args)
    if not options(args) and not fault:
        return
    if (args.payload != 'image' or args.mode != 'suite' or args.disk != 'root'
            or getattr(args, 'root_fs', None) != 'breenfs'
            or not args.suite or ',' in args.suite or args.suite == 'all'
            or getattr(args, 'session', False)):
        raise ValueError('power cuts need a single image suite on --disk root --root-fs breenfs')
    if getattr(args, 'nmi', False) or args.inject:
        raise ValueError('power cuts cannot be combined with --nmi or --inject')
    if fault and (args.suite != 'filesystem' or getattr(args, 'case', None) != 'power-cut/writer'
                  or getattr(args, 'kernel_command_line', None) is not None):
        raise ValueError('BreenFS faults need --suite filesystem --case power-cut/writer and supply their own kernel command line')
