# Kindle Wireless Sync - build, test and run

kindle_target := "armv7-unknown-linux-musleabihf"
# SSH host for the device recipes. `kindle` is the reference device; set
# KINDLE_HOST to use another, e.g. `kindle-emu` for an eink-emulator guest.
kindle_host := env("KINDLE_HOST", "kindle")
# On the device: /var/local is persistent, writable and
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

# Drive an isolated Calibre with the test harness plugin (ADR 0019), e.g. `just calibre launch`
[positional-arguments]
calibre *args:
    python3 tools/calibre-harness/harness.py "$@"

# Show the packaged Kindle binary size
size-kindle: build-kindle-package
    @ls -l target/{{kindle_target}}/release/kindle-wireless-sync | awk '{print $5 " bytes"}'

# Copy the Kindle binary to the device
deploy: build-kindle
    ssh {{kindle_host}} "mkdir -p {{kindle_dir}}/state"
    # Copy under a temporary name, then rename over the old binary.
    scp -O -q target/{{kindle_target}}/release/kindle-wireless-sync {{kindle_host}}:{{kindle_dir}}/kindle-wireless-sync.new
    ssh {{kindle_host}} "chmod 755 {{kindle_dir}}/kindle-wireless-sync.new && mv -f {{kindle_dir}}/kindle-wireless-sync.new {{kindle_dir}}/kindle-wireless-sync"

# Copy the service scripts (writable storage; no root remount)
deploy-scripts:
    scp -O -q package/service.sh package/autostart.sh {{kindle_host}}:{{kindle_dir}}/
    ssh {{kindle_host}} "chmod 755 {{kindle_dir}}/service.sh {{kindle_dir}}/autostart.sh"

# Install the upstart jobs (remounts the root filesystem read-write briefly)
install-service: deploy deploy-scripts
    scp -O -q package/upstart/kindle-wireless-sync.conf package/upstart/kindle-wireless-sync-autostart.conf {{kindle_host}}:/tmp/
    ssh {{kindle_host}} "trap '/usr/sbin/mntroot ro' EXIT; /usr/sbin/mntroot rw && cp /tmp/kindle-wireless-sync.conf /tmp/kindle-wireless-sync-autostart.conf /etc/upstart/ && rm -f /tmp/kindle-wireless-sync*.conf"
    ssh {{kindle_host}} "/sbin/initctl reload-configuration"

# Remove the upstart jobs (remounts the root filesystem read-write briefly)
uninstall-service:
    -ssh {{kindle_host}} "/sbin/initctl stop kindle-wireless-sync"
    ssh {{kindle_host}} "trap '/usr/sbin/mntroot ro' EXIT; /usr/sbin/mntroot rw && rm -f /etc/upstart/kindle-wireless-sync.conf /etc/upstart/kindle-wireless-sync-autostart.conf"
    ssh {{kindle_host}} "/sbin/initctl reload-configuration"

# Service control on the Kindle
start:
    ssh {{kindle_host}} "/sbin/initctl start kindle-wireless-sync"

stop:
    ssh {{kindle_host}} "/sbin/initctl stop kindle-wireless-sync"

restart:
    ssh {{kindle_host}} "/sbin/initctl restart kindle-wireless-sync"

status:
    ssh {{kindle_host}} "/sbin/initctl status kindle-wireless-sync; {{kindle_dir}}/kindle-wireless-sync status --state {{kindle_dir}}/state; [ -f {{kindle_dir}}/autostart ] && echo 'autostart: on' || echo 'autostart: off'"

logs:
    ssh {{kindle_host}} "tail -f /var/log/kindle-wireless-sync.log"

autostart-on:
    ssh {{kindle_host}} "touch {{kindle_dir}}/autostart && rm -f {{kindle_dir}}/boot_attempts"

autostart-off:
    ssh {{kindle_host}} "rm -f {{kindle_dir}}/autostart"

# Keep the Kindle awake for long tests (Wi-Fi drops with the screensaver).
# A reboot also turns the screensaver back on.
screensaver-off:
    ssh {{kindle_host}} "lipc-set-prop -i com.lab126.powerd preventScreenSaver 1 && lipc-get-prop com.lab126.powerd preventScreenSaver"

screensaver-on:
    ssh {{kindle_host}} "lipc-set-prop -i com.lab126.powerd preventScreenSaver 0 && lipc-get-prop com.lab126.powerd preventScreenSaver"

# Build the library-tile scriptlet with its icon embedded
scriptlet:
    #!/bin/sh
    set -eu
    mkdir -p target/package
    # Fail rather than embed an empty icon (as v0.1.0 shipped).
    icon=$(base64 < package/icon/icon.png | tr -d '\n')
    [ -n "$icon" ] || { echo "package/icon/icon.png is missing or empty" >&2; exit 1; }
    sed "s|@ICON@|$icon|" package/scriptlet.sh.in > target/package/WirelessSync.sh

# Deploy the WAF app and scriptlet, register the app, restart the service
deploy-ui: deploy deploy-scripts scriptlet
    ssh {{kindle_host}} "mkdir -p {{kindle_dir}}/ui"
    scp -O -q package/ui/* {{kindle_host}}:{{kindle_dir}}/ui/
    # mesquite runs as the framework user: the app must be readable (see install.sh).
    ssh {{kindle_host}} "chmod 755 {{kindle_dir}} {{kindle_dir}}/ui && chmod 644 {{kindle_dir}}/ui/*"
    scp -O -q package/register-app.sh {{kindle_host}}:{{kindle_dir}}/
    scp -O -q target/package/WirelessSync.sh {{kindle_host}}:/mnt/us/documents/WirelessSync.sh
    ssh {{kindle_host}} "chmod 755 {{kindle_dir}}/register-app.sh && sh {{kindle_dir}}/register-app.sh register && rm -rf /var/local/mesquite/com.kindlewirelesssync.app"
    -ssh {{kindle_host}} "/sbin/initctl restart kindle-wireless-sync || /sbin/initctl start kindle-wireless-sync"

# Build the KPM package: target/package/kindle-wireless-sync.kpkg (ADR 0012)
package: build-kindle-package scriptlet
    sh tools/package.sh target/{{kindle_target}}/release/kindle-wireless-sync \
        target/package/WirelessSync.sh target/package/kindle-wireless-sync.kpkg

# Prepare a release: bump the version, date the changelog, commit (ADR 0014)
release version:
    sh tools/release.sh prepare {{version}}

# Tag the prepared release and push it, which publishes it (asks first)
publish-release *flags:
    sh tools/release.sh publish {{flags}}

# Install the package with the device's own KPM, as a user would
kpm-install: package
    scp -O -q target/package/kindle-wireless-sync.kpkg {{kindle_host}}:/tmp/kindle-wireless-sync.kpkg
    ssh {{kindle_host}} "/var/local/kmc/bin/kpm -y install file:///tmp/kindle-wireless-sync.kpkg; rc=\$?; rm -f /tmp/kindle-wireless-sync.kpkg; exit \$rc"

# Uninstall the package with the device's own KPM
kpm-uninstall:
    ssh {{kindle_host}} "/var/local/kmc/bin/kpm -y uninstall kindle-wireless-sync"

# Open the settings app on the Kindle
open-ui:
    ssh {{kindle_host}} "lipc-set-prop com.lab126.appmgrd start app://com.kindlewirelesssync.app"

# Deploy, then run one session on the Kindle in the foreground
device-connect *args: deploy
    ssh -t {{kindle_host}} "RUST_LOG=${RUST_LOG:-debug} {{kindle_dir}}/kindle-wireless-sync connect --books {{kindle_books}} --state {{kindle_dir}}/state --thumbnails /mnt/us/system/thumbnails --catalog /var/local/cc.db {{args}}"

# Run discovery from the Kindle
device-discover: deploy
    ssh {{kindle_host}} "{{kindle_dir}}/kindle-wireless-sync discover"

# Dev: native series grouping on the Kindle: status (default), enable or disable. Not packaged.
series-grouping action="status":
    ssh {{kindle_host}} "sh -s -- {{action}}" < tools/series-grouping.sh
