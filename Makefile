# Entry points. The scripts hold the real logic; this file only names them.
.PHONY: all rust app release test test-rust test-swift lint fmt run clean

all: app

rust:
	scripts/build-rust.sh release

app:
	scripts/build-rust.sh debug
	scripts/bundle-app.sh debug

release:
	scripts/build-rust.sh release
	scripts/bundle-app.sh release

test: test-rust test-swift

test-rust:
	cargo test --workspace

test-swift:
	scripts/build-rust.sh debug
	swift test --package-path apps/macos

lint:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt

run: app
	open build/app/OpenShareMouse.app

clean:
	cargo clean
	rm -rf build apps/macos/.build apps/macos/Frameworks apps/macos/Sources/OSMCore/Generated
