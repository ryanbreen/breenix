#!/usr/bin/env bash
# Keep Mac builds in Darwin background CPU policy with utility disk I/O.
# taskpolicy backgrounds itself before POSIX_SPAWN_SETEXEC. Its default background
# disk policy therefore covers loading the command, before the command can log.
# Explicitly request the observed default utility policy; this is not a stall fix.
set -euo pipefail
if [ "$(uname -s)" = Darwin ]; then
    exec taskpolicy -b -g utility "$@"
fi
exec "$@"
