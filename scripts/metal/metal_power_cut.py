"""Command-line options shared by the host launcher and leased boot driver."""
import argparse
import math


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


def options(args):
    after = getattr(args, 'power_cut_after', None)
    window = getattr(args, 'power_cut_window', None)
    if after is None and window is None:
        return []
    return ['--power-cut-after', str(after)] if window is None else ['--power-cut-window', window]


def validate(args):
    if not options(args):
        return
    if (args.payload != 'image' or args.mode != 'suite' or args.disk != 'root'
            or getattr(args, 'root_fs', None) != 'breenfs'
            or not args.suite or ',' in args.suite or args.suite == 'all'
            or getattr(args, 'session', False)):
        raise ValueError('power cuts need a single image suite on --disk root --root-fs breenfs')
    if getattr(args, 'nmi', False) or args.inject:
        raise ValueError('power cuts cannot be combined with --nmi or --inject')
