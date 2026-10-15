# Kindle Wireless Sync - build, test and run

kindle_target := "armv7-unknown-linux-musleabihf"
# On the device (ssh host `kindle`): /var/local is persistent, writable and
# not exposed over USB (ADR 0004).
kindle_dir := "/var/local/kindle-wireless-sync"
kindle_books := "/mnt/us/documents"

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

# Copy the Kindle binary to the device
deploy: build-kindle
    ssh kindle "mkdir -p {{kindle_dir}}/state"
    # Copy under a temporary name, then rename over the old binary.
    scp -q target/{{kindle_target}}/release/kindle-wireless-sync kindle:{{kindle_dir}}/kindle-wireless-sync.new
    ssh kindle "chmod 755 {{kindle_dir}}/kindle-wireless-sync.new && mv -f {{kindle_dir}}/kindle-wireless-sync.new {{kindle_dir}}/kindle-wireless-sync"

# Deploy, then run one session on the Kindle in the foreground
device-connect *args: deploy
    ssh -t kindle "RUST_LOG=${RUST_LOG:-debug} {{kindle_dir}}/kindle-wireless-sync connect --books {{kindle_books}} --state {{kindle_dir}}/state --thumbnails /mnt/us/system/thumbnails --catalog /var/local/cc.db {{args}}"

# Run discovery from the Kindle
device-discover: deploy
    ssh kindle "{{kindle_dir}}/kindle-wireless-sync discover"
