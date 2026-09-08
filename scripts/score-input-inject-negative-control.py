#!/usr/bin/env python3
"""Require one actual INPUT_INJECT rejection from the production serial."""
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text(errors="replace")
records = re.findall(r"\[INPUT_INJECT_NEGATIVE_CONTROL:[^\]]*\]", text)
expected = "[INPUT_INJECT_NEGATIVE_CONTROL:request=0xb8130004:probe=-25:verdict=PASS]"
if records != [expected]:
    sys.exit(f"FAIL: INPUT_INJECT production rejection: {records}")
print(expected)
