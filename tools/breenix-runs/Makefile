APP_NAME := Breenix Run Inspector
APP_BUNDLE := $(APP_NAME).app
APP_EXECUTABLE := BreenixRunInspector
APP_CONTENTS := $(APP_BUNDLE)/Contents
APP_MACOS := $(APP_CONTENTS)/MacOS
APP_RESOURCES := $(APP_CONTENTS)/Resources
INFO_PLIST := $(APP_CONTENTS)/Info.plist

.PHONY: build test catalog app

build:
	swift build

test:
	swift test

catalog:
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch aarch64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch x86_64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json

app:
	# Debug, not release: `-c release` crashes this toolchain's swift-frontend
	# (Swift 6.3.3, swiftlang-6.3.3.1.3) -- a CopyPropagation SIL-pass ownership
	# assertion while compiling RunShow.renderSubsystems (PR-3,
	# Sources/BreenixRuns/Show/RunShow.swift:89) under -O. That function's logic
	# is correct (swift test covers it and is green) -- see DESIGN.md Sec 2.4
	# for the reproduction and why this stays a debug build rather than a
	# source change made only to dodge a compiler crash it did not cause.
	swift build --product $(APP_EXECUTABLE) -c debug
	rm -rf "$(APP_BUNDLE)"
	mkdir -p "$(APP_MACOS)" "$(APP_RESOURCES)"
	cp ".build/debug/$(APP_EXECUTABLE)" "$(APP_MACOS)/$(APP_EXECUTABLE)"
	{ \
		printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'; \
		printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'; \
		printf '%s\n' '<plist version="1.0">'; \
		printf '%s\n' '<dict>'; \
		printf '%s\n' '  <key>CFBundleExecutable</key>'; \
		printf '%s\n' '  <string>$(APP_EXECUTABLE)</string>'; \
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
