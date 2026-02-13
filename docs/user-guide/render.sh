#!/bin/bash
# Renders Markdown docs as styled HTML and opens in the default browser.
# Usage: ./render.sh [file.md]  (defaults to bsh-shell-guide.md)
#
# Features:
#   - Server-side JavaScript syntax highlighting (no CDN / no external deps)
#   - Proper indentation preserved in code blocks
#   - GitHub-style light/dark theme with matching code colors

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INPUT="${1:-$SCRIPT_DIR/bsh-shell-guide.md}"
OUTPUT="${INPUT%.md}.html"
TITLE=$(head -1 "$INPUT" | sed 's/^# //')

python3 - "$INPUT" "$TITLE" > "$OUTPUT" << 'PYEOF'
import sys, re, html as html_mod

# =============================================================================
# JavaScript tokenizer - produces (token_type, text) pairs
# =============================================================================

JS_KEYWORDS = frozenset({
    'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue',
    'default', 'delete', 'do', 'else', 'export', 'extends', 'finally', 'for',
    'function', 'if', 'import', 'in', 'instanceof', 'let', 'new', 'of',
    'return', 'switch', 'throw', 'try', 'typeof', 'var', 'void', 'while',
    'with', 'yield',
})

JS_LITERALS = frozenset({
    'true', 'false', 'null', 'undefined', 'NaN', 'Infinity',
})

JS_BUILTINS = frozenset({
    'console', 'JSON', 'Math', 'Number', 'Map', 'Set', 'Promise',
    'Array', 'Object', 'String', 'Error', 'parseInt', 'parseFloat',
    'RegExp', 'Date', 'Symbol',
})


def tokenize_js(code):
    """Yield (token_type, text) tuples for JavaScript source."""
    i = 0
    n = len(code)
    while i < n:
        c = code[i]

        # --- Whitespace ---
        if c in ' \t\n\r':
            j = i + 1
            while j < n and code[j] in ' \t\n\r':
                j += 1
            yield ('', code[i:j])
            i = j
            continue

        # --- Single-line comment ---
        if c == '/' and i + 1 < n and code[i + 1] == '/':
            j = code.find('\n', i)
            if j == -1:
                j = n
            yield ('comment', code[i:j])
            i = j
            continue

        # --- Multi-line comment ---
        if c == '/' and i + 1 < n and code[i + 1] == '*':
            j = code.find('*/', i + 2)
            if j == -1:
                j = n
            else:
                j += 2
            yield ('comment', code[i:j])
            i = j
            continue

        # --- Double-quoted string ---
        if c == '"':
            j = i + 1
            while j < n and code[j] != '"':
                if code[j] == '\\' and j + 1 < n:
                    j += 2
                else:
                    j += 1
            if j < n:
                j += 1
            yield ('string', code[i:j])
            i = j
            continue

        # --- Single-quoted string ---
        if c == "'":
            j = i + 1
            while j < n and code[j] != "'":
                if code[j] == '\\' and j + 1 < n:
                    j += 2
                else:
                    j += 1
            if j < n:
                j += 1
            yield ('string', code[i:j])
            i = j
            continue

        # --- Template literal ---
        if c == '`':
            yield ('string', '`')
            i += 1
            while i < n and code[i] != '`':
                if code[i] == '\\' and i + 1 < n:
                    yield ('string', code[i:i + 2])
                    i += 2
                elif code[i:i + 2] == '${':
                    yield ('subst', '${')
                    i += 2
                    depth = 1
                    start = i
                    while i < n and depth > 0:
                        if code[i] == '{':
                            depth += 1
                        elif code[i] == '}':
                            depth -= 1
                        if depth > 0:
                            i += 1
                    expr = code[start:i]
                    # Recursively tokenize the expression inside ${}
                    yield from tokenize_js(expr)
                    yield ('subst', '}')
                    if i < n:
                        i += 1  # skip closing }
                else:
                    j = i
                    while j < n and code[j] != '`' and code[j:j + 2] != '${' and code[j] != '\\':
                        j += 1
                    yield ('string', code[i:j])
                    i = j
            if i < n:
                yield ('string', '`')
                i += 1
            continue

        # --- Numbers ---
        if c.isdigit() or (c == '.' and i + 1 < n and code[i + 1].isdigit()):
            j = i
            if code[j:j + 2] in ('0x', '0X', '0b', '0B', '0o', '0O'):
                j += 2
            while j < n and (code[j].isalnum() or code[j] in '._'):
                j += 1
            yield ('number', code[i:j])
            i = j
            continue

        # --- Arrow operator ---
        if code[i:i + 2] == '=>':
            yield ('keyword', '=>')
            i += 2
            continue

        # --- Spread / rest ---
        if code[i:i + 3] == '...':
            yield ('keyword', '...')
            i += 3
            continue

        # --- Identifiers / keywords ---
        if c.isalpha() or c == '_' or c == '$':
            j = i + 1
            while j < n and (code[j].isalnum() or code[j] == '_' or code[j] == '$'):
                j += 1
            word = code[i:j]
            if word in JS_KEYWORDS:
                yield ('keyword', word)
            elif word in JS_LITERALS:
                yield ('literal', word)
            elif word in JS_BUILTINS:
                yield ('built_in', word)
            else:
                # Detect function call / definition: identifier followed by (
                k = j
                while k < n and code[k] in ' \t':
                    k += 1
                if k < n and code[k] == '(':
                    yield ('title', word)
                else:
                    yield ('', word)
            i = j
            continue

        # --- Everything else (operators, punctuation) ---
        yield ('', c)
        i += 1


def highlight_js(code):
    """Tokenize JS source and return HTML with <span> elements."""
    parts = []
    for typ, text in tokenize_js(code):
        escaped = html_mod.escape(text)
        if typ:
            parts.append(f'<span class="hl-{typ}">{escaped}</span>')
        else:
            parts.append(escaped)
    return ''.join(parts)


def highlight_bash(code):
    """Simple bash highlighter: comments and strings."""
    parts = []
    i = 0
    n = len(code)
    while i < n:
        c = code[i]
        # Comment (only if # is at start of line or after whitespace)
        if c == '#' and (i == 0 or code[i - 1] in ' \t\n'):
            j = code.find('\n', i)
            if j == -1:
                j = n
            parts.append(f'<span class="hl-comment">{html_mod.escape(code[i:j])}</span>')
            i = j
            continue
        # Double-quoted string
        if c == '"':
            j = i + 1
            while j < n and code[j] != '"':
                if code[j] == '\\' and j + 1 < n:
                    j += 2
                else:
                    j += 1
            if j < n:
                j += 1
            parts.append(f'<span class="hl-string">{html_mod.escape(code[i:j])}</span>')
            i = j
            continue
        # Single-quoted string
        if c == "'":
            j = i + 1
            while j < n and code[j] != "'":
                j += 1
            if j < n:
                j += 1
            parts.append(f'<span class="hl-string">{html_mod.escape(code[i:j])}</span>')
            i = j
            continue
        parts.append(html_mod.escape(c))
        i += 1
    return ''.join(parts)


def highlight_code(code, lang):
    """Dispatch to the right highlighter based on language."""
    if lang in ('javascript', 'js'):
        return highlight_js(code)
    elif lang in ('bash', 'sh', 'shell'):
        return highlight_bash(code)
    else:
        return html_mod.escape(code)


# =============================================================================
# Markdown → HTML converter
# =============================================================================

md = open(sys.argv[1]).read()
title = sys.argv[2]

# --- Phase 1: Extract fenced code blocks, replace with placeholders ---
code_blocks = []

def extract_code_block(m):
    lang = m.group(1) or ''
    code = m.group(2).rstrip()
    idx = len(code_blocks)
    highlighted = highlight_code(code, lang)
    code_blocks.append(highlighted)
    return f'\x00CODEBLOCK{idx}\x00'

md = re.sub(r'```(\w*)\n(.*?)```', extract_code_block, md, flags=re.DOTALL)

# --- Phase 2: Inline formatting ---

# Inline code (must come before bold/italic)
md = re.sub(r'`([^`]+)`', lambda m: f'<code>{html_mod.escape(m.group(1))}</code>', md)

# Bold and italic
md = re.sub(r'\*\*(.+?)\*\*', r'<strong>\1</strong>', md)
md = re.sub(r'\*(.+?)\*', r'<em>\1</em>', md)

# Headings (most specific first)
md = re.sub(r'^### (.+)$', r'<h3>\1</h3>', md, flags=re.MULTILINE)
md = re.sub(r'^## (.+)$', r'<h2>\1</h2>', md, flags=re.MULTILINE)
md = re.sub(r'^# (.+)$', r'<h1>\1</h1>', md, flags=re.MULTILINE)

# Horizontal rules
md = re.sub(r'^---+$', '<hr>', md, flags=re.MULTILINE)

# Tables
def convert_tables(text):
    pattern = re.compile(r'((?:^\|.+\|\n)+)', re.MULTILINE)
    def replace_table(m):
        rows = m.group(1).strip().split('\n')
        r = '<table>\n'
        for i, row in enumerate(rows):
            cells = [c.strip() for c in row.strip('|').split('|')]
            if i == 1 and all(set(c.strip()) <= set('- :') for c in cells):
                continue
            tag = 'th' if i == 0 else 'td'
            r += '<tr>' + ''.join(f'<{tag}>{c}</{tag}>' for c in cells) + '</tr>\n'
        r += '</table>\n'
        return r
    return pattern.sub(replace_table, text)
md = convert_tables(md)

# Lists
md = re.sub(r'^- (.+)$', r'<li>\1</li>', md, flags=re.MULTILINE)
md = re.sub(r'((?:<li>.*</li>\n)+)', r'<ul>\n\1</ul>\n', md)

# --- Phase 3: Wrap bare text in paragraphs ---
result = []
for line in md.split('\n'):
    s = line.strip()
    if s and not s.startswith('<') and not s.startswith('|') and not s.startswith('\x00CODEBLOCK'):
        result.append(f'<p>{s}</p>')
    else:
        result.append(line)
body = '\n'.join(result)

# --- Phase 4: Restore code blocks ---
for idx, highlighted_html in enumerate(code_blocks):
    block = f'<pre><code>{highlighted_html}</code></pre>'
    body = body.replace(f'\x00CODEBLOCK{idx}\x00', block)

# --- Phase 5: Output final HTML ---
print(f'''<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html_mod.escape(title)}</title>
<style>
/* ── Layout ────────────────────────────────────────────────────────── */
body {{
  max-width: 980px;
  margin: 0 auto;
  padding: 45px 28px;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  line-height: 1.6;
  color: #24292f;
  background: #fff;
}}

/* ── Code blocks ───────────────────────────────────────────────────── */
pre {{
  background: #0d1117;
  color: #e6edf3;
  padding: 16px 20px;
  border-radius: 8px;
  overflow-x: auto;
  line-height: 1.5;
  font-size: 14px;
  tab-size: 4;
  margin: 1.2em 0;
  border: 1px solid #30363d;
}}
pre code {{
  font-family: "SF Mono", "Fira Code", "Cascadia Code", "JetBrains Mono", Menlo, Consolas, monospace;
  font-size: 14px;
  background: none;
  padding: 0;
  color: inherit;
}}

/* ── Inline code ───────────────────────────────────────────────────── */
code {{
  font-family: "SF Mono", "Fira Code", "Cascadia Code", "JetBrains Mono", Menlo, Consolas, monospace;
  font-size: 14px;
}}
:not(pre) > code {{
  background: #eff1f3;
  padding: 2px 6px;
  border-radius: 4px;
  color: #24292f;
  font-size: 85%;
}}

/* ── Syntax highlighting (GitHub Dark) ─────────────────────────────── */
.hl-keyword  {{ color: #ff7b72; }}
.hl-string   {{ color: #a5d6ff; }}
.hl-comment  {{ color: #8b949e; font-style: italic; }}
.hl-number   {{ color: #79c0ff; }}
.hl-literal  {{ color: #79c0ff; }}
.hl-built_in {{ color: #ffa657; }}
.hl-title    {{ color: #d2a8ff; }}
.hl-subst    {{ color: #e6edf3; }}

/* ── Tables ────────────────────────────────────────────────────────── */
table {{
  border-collapse: collapse;
  width: 100%;
  margin: 1em 0;
}}
th, td {{
  border: 1px solid #d0d7de;
  padding: 8px 12px;
  text-align: left;
}}
th {{
  background: #f6f8fa;
  font-weight: 600;
}}

/* ── Headings ──────────────────────────────────────────────────────── */
h1 {{
  border-bottom: 2px solid #d0d7de;
  padding-bottom: 12px;
  font-size: 2em;
}}
h2 {{
  border-bottom: 1px solid #d0d7de;
  padding-bottom: 8px;
  margin-top: 2em;
  font-size: 1.5em;
}}
h3 {{
  margin-top: 1.5em;
  font-size: 1.25em;
}}

/* ── Other elements ────────────────────────────────────────────────── */
hr {{
  border: none;
  border-top: 1px solid #d0d7de;
  margin: 2em 0;
}}
ul {{
  padding-left: 2em;
}}
li {{
  margin: 0.25em 0;
}}
p {{
  margin: 0.8em 0;
}}

/* ── Dark mode overrides ───────────────────────────────────────────── */
@media (prefers-color-scheme: dark) {{
  body {{
    background: #0d1117;
    color: #e6edf3;
  }}
  :not(pre) > code {{
    background: #161b22;
    color: #e6edf3;
  }}
  th {{
    background: #161b22;
  }}
  th, td {{
    border-color: #30363d;
  }}
  h1, h2 {{
    border-color: #30363d;
  }}
  hr {{
    border-color: #30363d;
  }}
}}
</style>
</head>
<body>
{body}
</body>
</html>''')
PYEOF

echo "Generated: $OUTPUT"

if [ "$(uname)" = "Darwin" ]; then
    open "$OUTPUT"
elif command -v xdg-open &>/dev/null; then
    xdg-open "$OUTPUT"
else
    echo "Open $OUTPUT in your browser"
fi
