#!/bin/sh
# INSTALL owns the disk-image tool and its packaged entry point.
set -eu
install -m 755 "$SDK_ROOT/sdk/libexec/image.py" "$SDK_STAGE/share/breenix-sdk/libexec/image.py"
cat > "$SDK_STAGE/bin/breenix-image" <<'PY'
#!/usr/bin/env python3
from pathlib import Path
import runpy
runpy.run_path(str(Path(__file__).resolve().parent.parent / 'share/breenix-sdk/libexec/image.py'), run_name='__main__')
PY
chmod 755 "$SDK_STAGE/bin/breenix-image"
