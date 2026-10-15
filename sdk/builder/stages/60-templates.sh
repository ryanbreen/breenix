#!/bin/sh
# TEMPLATES owns project creation, builds and SDK documentation.
set -eu
install -d "$SDK_STAGE/bin" "$SDK_STAGE/share/breenix-sdk/libexec" \
    "$SDK_STAGE/share/breenix-sdk/templates" "$SDK_STAGE/share/doc/breenix-sdk"
for tool in new build; do
    install -m 755 "$SDK_ROOT/sdk/libexec/$tool.py" "$SDK_STAGE/share/breenix-sdk/libexec/$tool.py"
    cat > "$SDK_STAGE/bin/breenix-$tool" <<EOF
#!/usr/bin/env python3
import pathlib
import runpy
root = pathlib.Path(__file__).resolve().parent.parent
runpy.run_path(str(root / 'share/breenix-sdk/libexec/$tool.py'), run_name='__main__')
EOF
    chmod 755 "$SDK_STAGE/bin/breenix-$tool"
done
cp -R "$SDK_ROOT/sdk/templates/." "$SDK_STAGE/share/breenix-sdk/templates/"
for doc in "$SDK_ROOT"/docs/sdk/*.md; do
    install -m 644 "$doc" "$SDK_STAGE/share/doc/breenix-sdk/"
done
