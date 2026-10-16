#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Run by /etc/upstart/kindle-wireless-sync.conf. Kept on writable storage so
# it can change without remounting the root filesystem (ADR 0004).
DIR=/var/local/kindle-wireless-sync

# autostart.sh counts boots; a daemon still running after two minutes is
# stable, so reset the count.
( sleep 120; pgrep -f "[k]indle-wireless-sync daemon" >/dev/null && rm -f "$DIR/boot_attempts" ) &

export RUST_LOG="${RUST_LOG:-info}"
exec "$DIR/kindle-wireless-sync" daemon \
    --books /mnt/us/documents \
    --state "$DIR/state" \
    --thumbnails /mnt/us/system/thumbnails \
    --catalog /var/local/cc.db \
    >> /var/log/kindle-wireless-sync.log 2>&1
