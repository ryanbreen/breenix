"""Compatibility entry point for the Cargo-integrated reply lookup regression."""
from pathlib import Path
import subprocess

if __name__ == '__main__':
    subprocess.run(['cargo', 'test', '--test', 'console_reply_lock'],
                   cwd=Path(__file__).resolve().parents[1], check=True)
