.PHONY: build test catalog app

build:
	swift build

test:
	swift test

catalog:
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch aarch64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json
	cd ../.. && cargo run -p xtask -- dump-boot-stages --arch x86_64 --json > tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json

app:
	@echo "make app lands in PR-6 with the SwiftUI app bundle"
