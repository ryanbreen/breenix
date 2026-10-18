"""Recorder transport fixtures: no git or boot-store access to the operator's checkout/account."""
from pathlib import Path
import os


def isolated_commands(env, root):
    tools = Path(root) / 'fixture-tools'
    tools.mkdir(exist_ok=True)
    git = tools / 'git'
    git.write_text('''#!/bin/sh
case "$3" in
  status) printf '%s' "${FIXTURE_DIRTY:-}" ;;
  stash) printf '%s\\n' cccccccccccccccccccccccccccccccccccccccc ;;
  update-ref) printf '%s\\n' "$@" > "$FIXTURE_REF" ;;
  rev-parse) printf '%s\\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ;;
  *) exit 1 ;;
esac
''')
    git.chmod(0o755)
    python = tools / 'python3'
    python.write_text('#!/bin/sh\nprintf "%s\\n" "$FIXTURE_ACCOUNT_HOME"\n')
    python.chmod(0o755)
    env.update(PATH=str(tools) + os.pathsep + env.get('PATH', ''),
               FIXTURE_ACCOUNT_HOME=str(Path(root) / 'account-home'), FIXTURE_REF=str(Path(root) / 'kept-ref'))
    return env
