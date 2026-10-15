#!/bin/sh
# KPM install hook (ADR 0012). KPM runs it from the extracted package
# (/mnt/us/kmc/kpm/packages/kindle-wireless-sync/) on install and on upgrade.
# Every step is safe to repeat: if this fails, KPM keeps no record of the
# install, and the fix is to install again.
set -e

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

echo "Installing to $DIR"
mkdir -p "$DIR/state"
put "$PKG/kindle-wireless-sync" "$DIR/kindle-wireless-sync" 755
for f in service.sh autostart.sh register-app.sh; do
    put "$PKG/$f" "$DIR/$f" 755
done
rm -rf "$DIR/ui.new"
cp -r "$PKG/ui" "$DIR/ui.new"
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
put "$PKG/WirelessSync.sh" "$SCRIPTLET" 755

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
/sbin/initctl reload-configuration

if [ "$running" = 1 ]; then
    echo "Restarting the service"
    /sbin/initctl restart "$JOB"
elif [ "$fresh" = 1 ]; then
    echo "Starting the service"
    /sbin/initctl start "$JOB"
fi

echo "Installed. Open Wireless Sync from the Kindle library."
