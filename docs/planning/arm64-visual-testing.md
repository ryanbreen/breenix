# ARM64 Visual Terminal Testing Infrastructure

## Overview

This document describes the automated testing infrastructure for the ARM64 graphical terminal in Breenix OS. The system can send keystrokes to QEMU's graphical display (VirtIO keyboard) and verify what appears on screen.

## Research Findings

### Existing Patterns in Codebase

1. **x86_64 Interactive Testing** (`xtask/src/main.rs:interactive_test()`):
   - Uses QEMU monitor TCP interface on port 4444
   - Sends keystrokes via `sendkey` command
   - Verifies output by reading serial logs
   - Proven pattern that works reliably

2. **ARM64 Keyboard Testing** (`scripts/test-arm64-keyboard.py`):
   - Uses pexpect for serial interaction
   - Tests UART-based keyboard input
   - Works with `-nographic` mode

3. **Graphics Configuration** (`run.sh`, `scripts/run-arm64-graphics.sh`):
   - VirtIO GPU: `-device virtio-gpu-device`
   - VirtIO Keyboard: `-device virtio-keyboard-device`
   - Display options: Cocoa (macOS), SDL (Linux), VNC

### QEMU Capabilities

1. **Monitor Commands** ([QEMU Documentation](https://qemu.weilnetz.de/doc/5.2/system/monitor.html)):
   - `sendkey <key>` - Send keyboard input
   - `screendump <filename>` - Capture screen to PPM (PNG in QEMU 7.1+)
   - Accessible via TCP: `-monitor tcp:127.0.0.1:4445,server,nowait`

2. **VNC Server** ([QEMU VNC](https://www.qemu.org/docs/master/system/vnc-security.html)):
   - Enable with `-vnc :0` (port 5900)
   - Allows external VNC clients to connect
   - Provides remote display and input

### Python Libraries

1. **vncdotool** ([GitHub](https://github.com/sibson/vncdotool)):
   - VNC client library for automation
   - `client.keyPress()` for keystrokes
   - `client.captureScreen()` for screenshots
   - Built on Twisted framework

2. **pytesseract** ([PyPI](https://pypi.org/project/pytesseract/)):
   - Python wrapper for Tesseract OCR
   - Extracts text from images
   - Works with PIL/Pillow

## Implementation

Two approaches are provided:

### Approach 1: QEMU Monitor (Recommended)

**Script**: `scripts/test-arm64-visual.py`

Uses QEMU's monitor interface directly:
- More reliable than VNC
- Works completely headless
- No extra network protocol overhead
- Based on proven x86_64 xtask pattern

**How it works**:
1. Start QEMU with `-monitor tcp:127.0.0.1:4445,server,nowait`
2. Connect via TCP socket
3. Send keys with `sendkey <key>` command
4. Capture screen with `screendump <file>.ppm`
5. Extract text using Tesseract OCR
6. Verify expected output

### Approach 2: VNC (Alternative)

**Script**: `scripts/test-arm64-vnc.py`

Uses vncdotool for VNC-based interaction:
- Better for debugging (can watch test in VNC viewer)
- More natural keyboard interaction
- Can detect screen changes in real-time

**How it works**:
1. Start QEMU with `-vnc :0`
2. Connect using vncdotool API
3. Send keys with `client.keyPress()`
4. Capture screen with `client.captureScreen()`
5. OCR the captured image

## Installation

### Python Dependencies

```bash
pip install -r scripts/requirements-visual-test.txt
```

Or individually:
```bash
pip install pexpect pillow pytesseract  # For QEMU monitor approach
pip install vncdotool                    # For VNC approach (optional)
```

### System Dependencies

**macOS**:
```bash
brew install tesseract
```

**Linux**:
```bash
apt install tesseract-ocr
```

**QEMU** (required):
- `qemu-system-aarch64` must be in PATH

## Usage

### Basic Test

```bash
# Full test with kernel build
./scripts/test-arm64-visual.py

# Skip build, use existing kernel
./scripts/test-arm64-visual.py --no-build

# Verbose output (shows OCR results)
./scripts/test-arm64-visual.py --verbose

# Keep screenshot files for debugging
./scripts/test-arm64-visual.py --keep-screens
```

### VNC Test (Alternative)

```bash
./scripts/test-arm64-vnc.py --verbose --keep-screens
```

### Exit Codes

- `0` - SUCCESS: Visual terminal works correctly
- `1` - FAILURE: Test failed
- `2` - SKIP: Missing dependencies

## Architecture

```
QEMU Monitor Approach:
+------------------+      TCP:4445      +------------------+
|   Python Test    | <----------------> |   QEMU Monitor   |
|                  |     sendkey/       |                  |
|  - Send keys     |     screendump     |  - Process keys  |
|  - Capture screen|                    |  - Write PPM     |
|  - OCR text      |                    |                  |
+------------------+                    +------------------+
        |                                       |
        v                                       v
+------------------+                    +------------------+
| Tesseract OCR    |                    | VirtIO Keyboard  |
| - Image to text  |                    | - Input to guest |
+------------------+                    +------------------+

VNC Approach:
+------------------+      VNC:5900      +------------------+
|   Python Test    | <----------------> |   QEMU VNC       |
|                  |     RFB protocol   |                  |
|  - vncdotool API |                    |  - Display/Input |
+------------------+                    +------------------+
```

## Limitations

1. **OCR Accuracy**: Tesseract may struggle with:
   - Custom fonts
   - Low resolution displays
   - Unusual character rendering
   - Serial output falls back to serial log verification

2. **Timing**: Tests need adequate delays for:
   - QEMU startup
   - Kernel boot
   - Screen refresh after keystrokes

3. **Dependencies**: Requires:
   - Tesseract OCR binary
   - Python packages
   - QEMU with VirtIO support

## Future Improvements

1. **Reference Image Comparison**: Compare screenshots to known-good reference images instead of OCR

2. **Character-Level Verification**: Extract framebuffer directly and compare glyph patterns

3. **CI Integration**: Add to CI pipeline for automated visual testing

4. **Multi-Resolution Support**: Test different display resolutions

## References

- [QEMU Monitor Documentation](https://www.qemu.org/docs/master/system/monitor.html)
- [vncdotool Documentation](https://vncdotool.readthedocs.io/)
- [pytesseract Documentation](https://pypi.org/project/pytesseract/)
- [Tesseract OCR](https://github.com/tesseract-ocr/tesseract)
