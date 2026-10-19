#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# KPM uninstall hook (ADR 0012). KPM runs it from the package directory.
# Books received from Calibre, and their thumbnails, are the user's: keep them.

# On upgrade KPM runs this before installing the new version: keep settings,
# the linked library and the service. install.sh replaces the files.
[ "$1" = upgrade ] && exit 0

# Any failure exits non-zero, so KPM keeps the package and the user can retry.
set -e

PKG=$(cd "$(dirname "$0")" && pwd)
DIR=/var/local/kindle-wireless-sync
JOB=kindle-wireless-sync
APP_ID=com.kindlewirelesssync.app
SCRIPTLET=/mnt/us/documents/WirelessSync.sh

remounted=0
trap 'if [ "$remounted" = 1 ]; then /usr/sbin/mntroot ro; fi' EXIT
trap 'exit 130' INT TERM HUP

echo "Stopping Wireless Sync"
# A graceful unload: killing mesquite makes appmgrd report a crash.
lipc-set-prop com.lab126.appmgrd stop "app://$APP_ID" 2>/dev/null || true
/sbin/initctl stop "$JOB" 2>/dev/null || true
# Dissolve the series we created (ADR 0016), so books aren't left grouped by
# a removed app. Best effort: the books stay readable either way.
if [ -x "$DIR/kindle-wireless-sync" ]; then
    "$DIR/kindle-wireless-sync" series --books /mnt/us/documents --catalog /var/local/cc.db \
        --remove-all || echo "Could not remove series; continuing"
fi

for f in kindle-wireless-sync.conf kindle-wireless-sync-autostart.conf; do
    if [ -e "/etc/upstart/$f" ]; then
        if [ "$remounted" = 0 ]; then
            /usr/sbin/mntroot rw
            remounted=1
        fi
        echo "Removing /etc/upstart/$f"
        rm -f "/etc/upstart/$f"
    fi
done
if [ "$remounted" = 1 ]; then
    /usr/sbin/mntroot ro
    remounted=0
fi
# initctl can time out waiting for upstart's reply (seen on the emulated
# Kindle, 2026-10-17 and 2026-10-18); try once more. Here the reload only
# makes upstart forget jobs whose files are gone, so if it still fails, carry
# on: stopping now would leave the app registered (seen on 5.19.2,
# 2026-10-19).
/sbin/initctl reload-configuration || { sleep 2; /sbin/initctl reload-configuration; } ||
    echo "Couldn't reload upstart's jobs; the Kindle drops them at its next restart"

echo "Unregistering the Wireless Sync app"
# The package's own copy: it works even if $DIR is already gone.
sh "$PKG/register-app.sh" unregister
rm -f "$SCRIPTLET"
rm -rf "$SCRIPTLET.sdr"

echo "Removing $DIR"
rm -rf "$DIR"
rm -f /var/log/kindle-wireless-sync.log

echo "Uninstalled. Books received from Calibre are still in your library."
