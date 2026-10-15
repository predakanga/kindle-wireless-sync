#!/bin/sh
# Run once per boot by /etc/upstart/kindle-wireless-sync-autostart.conf.
# Starts the service if autostart is on. Crash-loop guard: after three boots
# without a stable run (see service.sh), autostart turns itself off.
DIR=/var/local/kindle-wireless-sync
[ -f "$DIR/autostart" ] || exit 0
[ -x "$DIR/kindle-wireless-sync" ] || exit 0

n=$(cat "$DIR/boot_attempts" 2>/dev/null || echo 0)
case "$n" in ''|*[!0-9]*) n=0 ;; esac
if [ "$n" -ge 3 ]; then
    echo "$(date) autostart disabled after $n boots without a stable run" >> "$DIR/state/boot.log"
    rm -f "$DIR/autostart" "$DIR/boot_attempts"
    exit 0
fi
echo $((n + 1)) > "$DIR/boot_attempts"
start -n kindle-wireless-sync
exit 0
