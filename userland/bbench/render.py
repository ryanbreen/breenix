"""Replace the results table in BASELINE.md and write the document to stdout."""

from pathlib import Path
import sys

import tables

START = "<!-- bbench results start -->"
END = "<!-- bbench results end -->"


def render(document, table):
    before, start, rest = document.partition(START)
    _, end, after = rest.partition(END)
    if not start or not end:
        raise ValueError("BASELINE.md is missing the bbench results markers")
    return f"{before}{START}\n{table}\n{END}{after}"


if __name__ == "__main__":
    parser = tables.arguments()
    parser.description = __doc__
    args = parser.parse_args()
    document = Path(__file__).with_name("BASELINE.md").read_text()
    sys.stdout.write(render(document, tables.table(args)))
