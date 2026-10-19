#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# KPM install hook (ADR 0012). KPM runs it from the extracted package
# (/mnt/us/kmc/kpm/packages/kindle-wireless-sync/) on install and on upgrade.
# Every step is safe to repeat: if this fails, KPM keeps no record of the
# install, and the fix is to install again.
set -e
# KPM runs this with the umask of the shell that ran `kpm`, which can be 077
# (some SSH servers). mesquite runs the app as the framework user, so with
# 077 the app's files are unreadable and it won't open ("Unsupported content
# path"). Set the modes we need instead: everything readable except state/,
# which holds library passwords. The chmods also repair earlier installs.
umask 022

PKG=$(cd "$(dirname "$0")" && pwd)
DIR=/var/local/kindle-wireless-sync
JOB=kindle-wireless-sync
APP_ID=com.kindlewirelesssync.app
SCRIPTLET=/mnt/us/documents/WirelessSync.sh

remounted=0
trap 'if [ "$remounted" = 1 ]; then /usr/sbin/mntroot ro; fi' EXIT
trap 'exit 130' INT TERM HUP

# Copy under a temporary name, then rename: safe while the service runs the
# binary or a shell reads the script.
put() {
    cp "$1" "$2.new"
    chmod "$3" "$2.new"
    mv -f "$2.new" "$2"
}

fresh=1
[ -x "$DIR/kindle-wireless-sync" ] && fresh=0
running=0
/sbin/initctl status "$JOB" 2>/dev/null | grep -q running && running=1

# A backgrounded app keeps its page in memory, so after an upgrade reopening
# it would run the old script against the new service. Close it the graceful
# way (killing mesquite makes appmgrd report a crash).
lipc-set-prop com.lab126.appmgrd stop "app://$APP_ID" 2>/dev/null || true

echo "Installing to $DIR"
mkdir -p "$DIR/state"
chmod 755 "$DIR"
chmod 700 "$DIR/state"
put "$PKG/kindle-wireless-sync" "$DIR/kindle-wireless-sync" 755
for f in service.sh autostart.sh register-app.sh; do
    put "$PKG/$f" "$DIR/$f" 755
done
rm -rf "$DIR/ui.new"
cp -r "$PKG/ui" "$DIR/ui.new"
chmod 755 "$DIR/ui.new"
chmod 644 "$DIR/ui.new"/*
rm -rf "$DIR/ui"
mv "$DIR/ui.new" "$DIR/ui"
# The crash-loop counter belongs to the old binary (ADR 0010). The autostart
# sentinel is left as it is: off on a fresh install, the user's choice on an
# upgrade.
rm -f "$DIR/boot_attempts"

echo "Registering the Wireless Sync app"
sh "$DIR/register-app.sh" register
# mesquite caches the app's files; without this an upgrade shows the old UI.
rm -rf "/var/local/mesquite/$APP_ID"
# Only when it changed: the scanner indexes it again each time (see the end).
cmp -s "$PKG/WirelessSync.sh" "$SCRIPTLET" || put "$PKG/WirelessSync.sh" "$SCRIPTLET" 755

# The root filesystem is read-only: remount only if a job file changed (ADR 0004).
for f in kindle-wireless-sync.conf kindle-wireless-sync-autostart.conf; do
    if ! cmp -s "$PKG/upstart/$f" "/etc/upstart/$f"; then
        if [ "$remounted" = 0 ]; then
            /usr/sbin/mntroot rw
            remounted=1
        fi
        echo "Installing /etc/upstart/$f"
        cp "$PKG/upstart/$f" "/etc/upstart/$f"
    fi
done
if [ "$remounted" = 1 ]; then
    /usr/sbin/mntroot ro
    remounted=0
fi
# upstart only sees a new or changed job after a reload.
# initctl can time out waiting for upstart's reply (seen on the emulated
# Kindle, 2026-10-17 and 2026-10-18); try once more before giving up.
/sbin/initctl reload-configuration || { sleep 2; /sbin/initctl reload-configuration; }

if [ "$running" = 1 ]; then
    echo "Restarting the service"
    /sbin/initctl restart "$JOB"
elif [ "$fresh" = 1 ]; then
    echo "Starting the service"
    # The check above can miss a running service (initctl's reply lost):
    # then start fails with "Job is already running", so restart it to run
    # the new binary (seen on the emulated Kindle, 2026-10-19).
    /sbin/initctl start "$JOB" || /sbin/initctl restart "$JOB"
fi

# KPM rescans the library as soon as this hook returns. If the rescan finds
# the scriptlet before the scanner has added it, 5.19.2 queues it twice; the
# second insert fails, and the scanner retries that batch until a reboot,
# so books in new folders aren't added meanwhile (progress log, Phase 6).
# Wait until the catalog has the scriptlet as it is now. First make sure
# it's there: once, on the emulated Kindle, the old copy the check above
# kept was gone by now (2026-10-19, cause unknown).
[ -x "$SCRIPTLET" ] || put "$PKG/WirelessSync.sh" "$SCRIPTLET" 755
mtime=$(stat -c %Y "$SCRIPTLET" 2>/dev/null || echo 0)
i=0
until [ "$(sqlite3 -readonly /var/local/cc.db \
        "SELECT count(*) FROM Entries WHERE p_location = '$SCRIPTLET' AND p_modificationTime >= $mtime;" \
        2>/dev/null)" = 1 ]; do
    i=$((i + 1))
    if [ "$i" -gt 30 ]; then
        echo "The library hasn't listed Wireless Sync yet; it should appear shortly"
        break
    fi
    sleep 1
done

echo "Installed. Open Wireless Sync from the Kindle library."
