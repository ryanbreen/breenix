APP_NAME := Breenix Run Inspector
APP_BUNDLE := $(APP_NAME).app
APP_EXECUTABLE := BreenixRunInspector
APP_CONTENTS := $(APP_BUNDLE)/Contents
APP_MACOS := $(APP_CONTENTS)/MacOS
APP_RESOURCES := $(APP_CONTENTS)/Resources
INFO_PLIST := $(APP_CONTENTS)/Info.plist

# The committed app icon (see `icon` target below and Icon/generate-icon.swift).
ICONSET := Icon/AppIcon.iconset
ICNS := Icon/AppIcon.icns
ICNS_NAME := AppIcon.icns

# Overridden via `$(MAKE) bundle CONFIG=...` by the `app`/`app-debug` targets
# below; not meant to be set directly on the command line.
CONFIG ?= release

.PHONY: build test catalog icon app app-debug bundle

build:
	swift build

test:
	swift test

catalog:
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch aarch64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch x86_64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json

# Regenerates Icon/AppIcon.icns from Icon/generate-icon.swift. The .icns is
# committed (it is what `bundle` copies into the app), so this target only
# needs to be re-run when the icon design itself changes; the generator is
# pure CoreGraphics drawing over literal constants, so re-running it
# reproduces Icon/AppIcon.icns byte-for-byte (verified: two consecutive runs
# on this toolchain produce identical PNGs and an identical .icns sha256).
icon:
	rm -rf "$(ICONSET)"
	mkdir -p "$(ICONSET)"
	swift Icon/generate-icon.swift "$(ICONSET)"
	iconutil -c icns "$(ICONSET)" -o "$(ICNS)"
	rm -rf "$(ICONSET)"

# Ships a release build (issue #866). `swift build -c release` used to crash
# this toolchain's swift-frontend (Swift 6.3.3, swiftlang-6.3.3.1.3) -- a
# CopyPropagation SIL-pass ownership assertion while compiling
# RunShow.renderSubsystems under -O. That function was restructured
# (Sources/BreenixRuns/Show/RunShow.swift) to avoid the SIL shape that
# tripped the pass, with no behavior change (`swift test` is still green);
# see DESIGN.md Sec 2.4 for the crash signature and the fix.
app:
	$(MAKE) bundle CONFIG=release

# Debug build of the same bundle, for a faster edit/build/launch loop.
app-debug:
	$(MAKE) bundle CONFIG=debug

bundle:
	swift build --product $(APP_EXECUTABLE) -c $(CONFIG)
	rm -rf "$(APP_BUNDLE)"
	mkdir -p "$(APP_MACOS)" "$(APP_RESOURCES)"
	cp ".build/$(CONFIG)/$(APP_EXECUTABLE)" "$(APP_MACOS)/$(APP_EXECUTABLE)"
	cp "$(ICNS)" "$(APP_RESOURCES)/$(ICNS_NAME)"
	{ \
		printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'; \
		printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'; \
		printf '%s\n' '<plist version="1.0">'; \
		printf '%s\n' '<dict>'; \
		printf '%s\n' '  <key>CFBundleExecutable</key>'; \
		printf '%s\n' '  <string>$(APP_EXECUTABLE)</string>'; \
		printf '%s\n' '  <key>CFBundleIconFile</key>'; \
		printf '%s\n' '  <string>AppIcon</string>'; \
		printf '%s\n' '  <key>CFBundleIdentifier</key>'; \
		printf '%s\n' '  <string>dev.breenix.run-inspector</string>'; \
		printf '%s\n' '  <key>CFBundleName</key>'; \
		printf '%s\n' '  <string>$(APP_NAME)</string>'; \
		printf '%s\n' '  <key>CFBundlePackageType</key>'; \
		printf '%s\n' '  <string>APPL</string>'; \
		printf '%s\n' '  <key>CFBundleShortVersionString</key>'; \
		printf '%s\n' '  <string>1.0</string>'; \
		printf '%s\n' '  <key>CFBundleVersion</key>'; \
		printf '%s\n' '  <string>1</string>'; \
		printf '%s\n' '  <key>LSMinimumSystemVersion</key>'; \
		printf '%s\n' '  <string>14.0</string>'; \
		printf '%s\n' '  <key>NSHighResolutionCapable</key>'; \
		printf '%s\n' '  <true/>'; \
		printf '%s\n' '</dict>'; \
		printf '%s\n' '</plist>'; \
	} > "$(INFO_PLIST)"
	codesign --force --deep --sign - "$(APP_BUNDLE)"
