#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Take a screenshot on the Kindle and fetch it.
# Usage: screenshot.sh HOST [OUT]
#
# Runs the stock /usr/sbin/screenshot, which writes a PNG to
# /mnt/us/screenshots/, copies the new file to OUT (default
# target/screens/<time>.png) and deletes it on the device. Uses cat over SSH
# rather than scp, which has failed on the Kindle while SSH worked.
set -eu

host=${1:?usage: screenshot.sh HOST [OUT]}
out=${2:-}
[ -n "$out" ] || out=target/screens/$(date +%Y%m%d-%H%M%S).png
mkdir -p "$(dirname "$out")"

# The newest PNG after the screenshot, if it wasn't there before.
remote=$(ssh "$host" 'dir=/mnt/us/screenshots
before=$(ls -t "$dir"/*.png 2>/dev/null | head -n 1)
/usr/sbin/screenshot >/dev/null 2>&1 || { echo "screenshot failed" >&2; exit 1; }
after=$(ls -t "$dir"/*.png 2>/dev/null | head -n 1)
[ -n "$after" ] && [ "$after" != "$before" ] || { echo "no new screenshot in $dir" >&2; exit 1; }
echo "$after"')

ssh "$host" "cat '$remote' && rm -f '$remote'" > "$out"
echo "$out"
