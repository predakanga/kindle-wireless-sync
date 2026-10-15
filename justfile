# Kindle Wireless Sync - build, test and run

kindle_target := "armv7-unknown-linux-musleabihf"

default:
    @just --list

# Build for the host
build:
    cargo build

# Build the static Kindle binary
build-kindle:
    cargo build --release --target {{kindle_target}}

# Run all tests
test:
    cargo test

# Run tests whose name matches a filter, e.g. `just test-one lpath`
test-one filter:
    cargo test {{filter}}

# Format and lint
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

fmt:
    cargo fmt

# List Calibre servers answering wireless-device discovery
discover *args:
    RUST_LOG=info cargo run -- discover {{args}}

# Connect to Calibre on the host, storing books in ./dev-device
connect *args:
    mkdir -p dev-device/books dev-device/state
    RUST_LOG=debug cargo run -- connect --books dev-device/books --state dev-device/state {{args}}

# Show the Kindle binary size
size-kindle: build-kindle
    @ls -l target/{{kindle_target}}/release/kindle-wireless-sync | awk '{print $5 " bytes"}'
