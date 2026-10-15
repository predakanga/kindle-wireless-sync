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
    cargo build --features dev-tools

# Build the static Kindle binary for development (includes dev tools)
build-kindle:
    cargo build --release --target {{kindle_target}} --features dev-tools

# Build the static Kindle binary as packaged (no dev tools)
build-kindle-package:
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
    cargo clippy --all-targets --features dev-tools -- -D warnings

fmt:
    cargo fmt

# List Calibre servers answering wireless-device discovery
discover *args:
    RUST_LOG=info cargo run --features dev-tools -- discover {{args}}

# Connect to Calibre on the host, storing books in ./dev-device
connect *args:
    mkdir -p dev-device/books dev-device/state
    RUST_LOG=debug cargo run --features dev-tools -- connect --books dev-device/books --state dev-device/state {{args}}

# Show the packaged Kindle binary size
size-kindle: build-kindle-package
    @ls -l target/{{kindle_target}}/release/kindle-wireless-sync | awk '{print $5 " bytes"}'

# Copy the Kindle binary to the device
deploy: build-kindle
    ssh kindle "mkdir -p {{kindle_dir}}/state"
    # Copy under a temporary name, then rename over the old binary.
    scp -q target/{{kindle_target}}/release/kindle-wireless-sync kindle:{{kindle_dir}}/kindle-wireless-sync.new
    ssh kindle "chmod 755 {{kindle_dir}}/kindle-wireless-sync.new && mv -f {{kindle_dir}}/kindle-wireless-sync.new {{kindle_dir}}/kindle-wireless-sync"

# Copy the service scripts (writable storage; no root remount)
deploy-scripts:
    scp -q package/service.sh package/autostart.sh kindle:{{kindle_dir}}/
    ssh kindle "chmod 755 {{kindle_dir}}/service.sh {{kindle_dir}}/autostart.sh"

# Install the upstart jobs (remounts the root filesystem read-write briefly)
install-service: deploy deploy-scripts
    scp -q package/upstart/kindle-wireless-sync.conf package/upstart/kindle-wireless-sync-autostart.conf kindle:/tmp/
    ssh kindle "trap '/usr/sbin/mntroot ro' EXIT; /usr/sbin/mntroot rw && cp /tmp/kindle-wireless-sync.conf /tmp/kindle-wireless-sync-autostart.conf /etc/upstart/ && rm -f /tmp/kindle-wireless-sync*.conf"
    ssh kindle "/sbin/initctl reload-configuration"

# Remove the upstart jobs (remounts the root filesystem read-write briefly)
uninstall-service:
    -ssh kindle "/sbin/initctl stop kindle-wireless-sync"
    ssh kindle "trap '/usr/sbin/mntroot ro' EXIT; /usr/sbin/mntroot rw && rm -f /etc/upstart/kindle-wireless-sync.conf /etc/upstart/kindle-wireless-sync-autostart.conf"
    ssh kindle "/sbin/initctl reload-configuration"

# Service control on the Kindle
start:
    ssh kindle "/sbin/initctl start kindle-wireless-sync"

stop:
    ssh kindle "/sbin/initctl stop kindle-wireless-sync"

restart:
    ssh kindle "/sbin/initctl restart kindle-wireless-sync"

status:
    ssh kindle "/sbin/initctl status kindle-wireless-sync; {{kindle_dir}}/kindle-wireless-sync status --state {{kindle_dir}}/state; [ -f {{kindle_dir}}/autostart ] && echo 'autostart: on' || echo 'autostart: off'"

logs:
    ssh kindle "tail -f /var/log/kindle-wireless-sync.log"

autostart-on:
    ssh kindle "touch {{kindle_dir}}/autostart && rm -f {{kindle_dir}}/boot_attempts"

autostart-off:
    ssh kindle "rm -f {{kindle_dir}}/autostart"

# Build the library-tile scriptlet with its icon embedded
scriptlet:
    mkdir -p target/package
    sed "s|@ICON@|$(base64 < package/icon/icon.png | tr -d '\n')|" package/scriptlet.sh.in > target/package/WirelessSync.sh

# Deploy the WAF app and scriptlet, register the app, restart the service
deploy-ui: deploy deploy-scripts scriptlet
    ssh kindle "mkdir -p {{kindle_dir}}/ui"
    scp -q package/ui/* kindle:{{kindle_dir}}/ui/
    scp -q package/register-app.sh kindle:{{kindle_dir}}/
    scp -q target/package/WirelessSync.sh kindle:/mnt/us/documents/WirelessSync.sh
    ssh kindle "chmod 755 {{kindle_dir}}/register-app.sh && sh {{kindle_dir}}/register-app.sh register && rm -rf /var/local/mesquite/com.kindlewirelesssync.app"
    -ssh kindle "/sbin/initctl restart kindle-wireless-sync || /sbin/initctl start kindle-wireless-sync"

# Build the KPM package: target/package/kindle-wireless-sync.kpkg (ADR 0012)
package: build-kindle-package scriptlet
    sh tools/package.sh target/{{kindle_target}}/release/kindle-wireless-sync \
        target/package/WirelessSync.sh target/package/kindle-wireless-sync.kpkg

# Install the package with the device's own KPM, as a user would
kpm-install: package
    scp -q target/package/kindle-wireless-sync.kpkg kindle:/tmp/kindle-wireless-sync.kpkg
    ssh kindle "/var/local/kmc/bin/kpm -y install file:///tmp/kindle-wireless-sync.kpkg; rc=\$?; rm -f /tmp/kindle-wireless-sync.kpkg; exit \$rc"

# Uninstall the package with the device's own KPM
kpm-uninstall:
    ssh kindle "/var/local/kmc/bin/kpm -y uninstall kindle-wireless-sync"

# Open the settings app on the Kindle
open-ui:
    ssh kindle "lipc-set-prop com.lab126.appmgrd start app://com.kindlewirelesssync.app"

# Deploy, then run one session on the Kindle in the foreground
device-connect *args: deploy
    ssh -t kindle "RUST_LOG=${RUST_LOG:-debug} {{kindle_dir}}/kindle-wireless-sync connect --books {{kindle_books}} --state {{kindle_dir}}/state --thumbnails /mnt/us/system/thumbnails --catalog /var/local/cc.db {{args}}"

# Run discovery from the Kindle
device-discover: deploy
    ssh kindle "{{kindle_dir}}/kindle-wireless-sync discover"
