#!/usr/bin/env python3
"""Run the cconf fixtures (N1570 translation phases 1-4 and hello semantics) against a compiler.

    run.py --cc /abs/cc --mode E       [--target x86_64|aarch64] [--stage a1 ...]
    run.py --cc /abs/cc --mode syntax  [...]
    run.py --cc /abs/cc --mode types --types-flag FLAG [...]
    run.py --list [--mode M]

Each fixture prints `cconf::<id> PASS` or `cconf::<id> FAIL (reason)`. The exit status
is 0 when every selected fixture passes, 1 when one fails, and 2 for a usage error or
a compiler path that does not name an executable file. README.md gives the contract.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys


HERE = Path(__file__).resolve().parent
TIMEOUT = 10
TARGETS = ("x86_64", "aarch64")
MODE_FLAGS = {"E": ["-E"], "syntax": ["-fsyntax-only"], "types": ["-fsyntax-only"]}
TYPE_FIELDS = ("type", "converted")

# N1570 6.4.6p1, longest first so the scan takes the longest punctuator (6.4p4).
PUNCTUATORS = sorted("""
    [ ] ( ) { } . -> ++ -- & * + - ~ ! / % << >> < > <= >= == != ^ | && || ? : ; ...
    = *= /= %= += -= <<= >>= &= ^= |= , # ## <: :> <% %> %: %:%:
""".split(), key=len, reverse=True)
UCN = r"\\u[0-9A-Fa-f]{4}|\\U[0-9A-Fa-f]{8}"
# 6.4.2.1: identifier-nondigit is a nondigit or a universal character name.
IDENTIFIER = re.compile(rf"(?:[A-Za-z_]|{UCN})(?:[A-Za-z0-9_]|{UCN})*")
# 6.4.8p1: digit or . digit, then digits, identifier-nondigits, e/E/p/P signs and periods.
PP_NUMBER = re.compile(rf"\.?[0-9](?:[eEpP][+-]|[A-Za-z0-9_.]|{UCN})*")
# 6.4.5p1 encoding prefixes for strings, 6.4.4.4p1 for character constants.
STRING_PREFIX = re.compile(r"(?:u8|u|U|L)?\"")
CHAR_PREFIX = re.compile(r"(?:u|U|L)?'")
DIAGNOSTIC = re.compile(
    r"^(?P<file>.+?):(?P<line>\d+):(?P<column>\d+): (?P<severity>fatal error|error|warning|"
    r"note|remark): (?P<message>.*)$")


def quoted_end(text, quote):
    """End of the quoted literal whose opening quote is at index quote, or None."""
    index = quote + 1
    while index < len(text):
        char = text[index]
        if char == "\\":
            index += 2
        elif char == text[quote]:
            return index + 1
        elif char == "\n":
            return None
        else:
            index += 1
    return None


def pp_tokens(text):
    """Split text into N1570 6.4 preprocessing tokens; header names are not formed."""
    tokens = []
    index = 0
    while index < len(text):
        char = text[index]
        if char in " \t\n\v\f\r":
            index += 1
            continue
        end = None
        for prefix in (STRING_PREFIX, CHAR_PREFIX):
            match = prefix.match(text, index)
            if match:
                end = quoted_end(text, match.end() - 1)
                break
        if end is None:
            match = PP_NUMBER.match(text, index) or IDENTIFIER.match(text, index)
            if match:
                end = match.end()
        if end is None:
            punctuator = next((p for p in PUNCTUATORS if text.startswith(p, index)), None)
            end = index + len(punctuator) if punctuator else index + 1
        tokens.append(text[index:end])
        index = end
    return tokens


def output_items(text):
    """The comparable content of -E output: pp-tokens in order, each `#pragma` line as one
    ("#pragma", tokens) item, and line markers (`# N ...`, `#line ...`) dropped."""
    items = []
    for line in text.split("\n"):
        tokens = pp_tokens(line)
        if len(tokens) >= 2 and tokens[0] in ("#", "%:"):
            if tokens[1][0].isdigit() or tokens[1] == "line":
                continue
            if tokens[1] == "pragma":
                items.append(("#pragma", tuple(tokens[2:])))
                continue
        items.extend(tokens)
    return items


def load_manifest(path):
    with open(path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    return manifest["cases"]


def diagnostics(stderr):
    return [match.groupdict() for match in map(DIAGNOSTIC.match, stderr.splitlines())
            if match]


def errors(stderr):
    return [d for d in diagnostics(stderr) if d["severity"] in ("error", "fatal error")]


def same_file(reported, expected):
    return reported == expected or reported.endswith("/" + expected)


def show(items, limit=12):
    return " ".join(item if isinstance(item, str) else f"[#pragma {' '.join(item[1])}]"
                    for item in items[:limit]) + (" ..." if len(items) > limit else "")


def first_difference(got, wanted):
    index = next((i for i, (a, b) in enumerate(zip(got, wanted)) if a != b),
                 min(len(got), len(wanted)))
    return (f"item {index}: got {show(got[index:], 6)!r}, wanted {show(wanted[index:], 6)!r}"
            f" ({len(got)} items, wanted {len(wanted)})")


def judge_negative(case, result):
    expected = case["diagnostic"]
    where = f"{expected['file']}:{expected['line']}:{expected['column']}"
    found = errors(result.stderr)
    if not found:
        return f"no error diagnostic, wanted one at {where} (exit {result.returncode})"
    if result.returncode == 0:
        return f"exit 0 after an error diagnostic at {where}"
    first = found[0]
    if not (same_file(first["file"], expected["file"]) and int(first["line"]) == expected["line"]
            and int(first["column"]) == expected["column"]):
        return (f"first error at {first['file']}:{first['line']}:{first['column']},"
                f" wanted {where}")
    if expected.get("contains") and expected["contains"] not in first["message"]:
        return f"message {first['message']!r} lacks {expected['contains']!r}"
    return None


def judge_preprocessed(case, result):
    got = output_items(result.stdout)
    attempts = []
    for expect in case["expect"]:
        wanted = output_items((HERE / expect).read_text(encoding="utf-8"))
        if got == wanted:
            return None
        attempts.append(f"{expect}: {first_difference(got, wanted)}")
    return "; ".join(attempts)


def type_tokens(value):
    if isinstance(value, list):
        return [type_tokens(item) for item in value]
    return pp_tokens(value) if isinstance(value, str) else value


def judge_types(case, result):
    records = []
    for number, line in enumerate(result.stdout.splitlines(), 1):
        if line.strip():
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError as error:
                return f"type dump line {number} is not JSON: {error}"
    wanted = json.loads((HERE / case["expect"][0]).read_text(encoding="utf-8"))["records"]
    for record in wanted:
        key = ("what", "line", "col", "name" if record["what"] == "decl" else "form")
        label = f"{record['what']} {record.get('name', record.get('form'))} at" \
                f" {record['line']}:{record['col']}"
        match = [r for r in records if all(r.get(k) == record[k] for k in key)]
        if len(match) != 1:
            return f"{label}: {len(match)} matching records in the dump"
        for field, value in record.items():
            if field == "clause":
                continue
            got = match[0].get(field)
            if field in TYPE_FIELDS or field == "args":
                same = got is not None and type_tokens(got) == type_tokens(value)
            else:
                same = got == value
            if not same:
                return f"{label}: {field} is {got!r}, wanted {value!r}"
    return None


def run_case(case, args):
    command = [str(args.cc), *args.cc_arg, *MODE_FLAGS[args.mode]]
    if args.mode == "types":
        command.append(args.types_flag)
    command += [*case.get("args", []), case["source"]]
    try:
        result = subprocess.run(command, cwd=HERE, capture_output=True, text=True,
                                errors="replace", timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return f"compiler ran longer than {TIMEOUT} s"
    except OSError as error:
        return f"compiler did not start: {error}"
    if result.returncode < 0:
        return f"compiler ended by signal {-result.returncode}"
    if "diagnostic" in case:
        return judge_negative(case, result)
    found = errors(result.stderr)
    if result.returncode != 0 or found:
        detail = f"{found[0]['file']}:{found[0]['line']}:{found[0]['column']}: " \
                 f"{found[0]['message']}" if found else result.stderr.strip()[:200]
        return f"exit {result.returncode} on an accepted fixture ({detail})"
    if args.mode == "E":
        return judge_preprocessed(case, result)
    if args.mode == "types":
        return judge_types(case, result)
    return None


def selected(cases, args):
    return [case for case in cases
            if case["mode"] == args.mode
            and args.target in case.get("targets", TARGETS)
            and (not args.stage or case["stage"] in args.stage)]


def joined_values(argv):
    """Attach the value of --cc-arg and --types-flag, which always looks like an option."""
    result = []
    words = iter(argv)
    for word in words:
        if word in ("--cc-arg", "--types-flag"):
            word = f"{word}={next(words, '')}"
        result.append(word)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cc", type=Path, help="absolute path of the compiler under test")
    parser.add_argument("--mode", choices=sorted(MODE_FLAGS), default="E")
    parser.add_argument("--target", choices=TARGETS, default="x86_64")
    parser.add_argument("--stage", action="append", choices=("a1", "a2", "b1"))
    parser.add_argument("--cc-arg", action="append", default=[],
                        help="argument placed before the mode flag; repeat as needed")
    parser.add_argument("--types-flag", help="option that makes the compiler print its type"
                        " records (types mode)")
    parser.add_argument("--list", action="store_true", help="list the selected fixtures")
    args = parser.parse_args(joined_values(sys.argv[1:]))

    cases = load_manifest(HERE / "cases.json")
    if args.list:
        for case in selected(cases, args):
            print(f"{case['id']}\t{case['stage']}\t{case['source']}")
        return
    if args.cc is None or not args.cc.is_absolute():
        parser.error("--cc must be an absolute path")
    if not (args.cc.is_file() and os.access(args.cc, os.X_OK)):
        parser.exit(2, f"cconf: compiler not found or not executable: {args.cc}\n")
    if args.mode == "types" and not args.types_flag:
        parser.error("--mode types needs --types-flag")

    chosen = selected(cases, args)
    if not chosen:
        parser.exit(2, "cconf: no fixtures selected\n")
    failed = 0
    for case in chosen:
        reason = run_case(case, args)
        if reason:
            failed += 1
            print(f"cconf::{case['id']} FAIL ({reason})", flush=True)
        else:
            print(f"cconf::{case['id']} PASS", flush=True)
    print(f"cconf: {len(chosen) - failed} of {len(chosen)} passed"
          f" (mode {args.mode}, target {args.target})")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
