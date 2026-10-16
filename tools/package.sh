#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the KPM package (ADR 0012, 0014) from an already built binary and
# scriptlet. Used by `just package` on macOS and by the release workflow on
# Linux, so it handles both bsdtar and GNU tar.
#
# Usage: tools/package.sh BINARY SCRIPTLET OUT.kpkg
set -eu

bin=$1
scriptlet=$2
out=$3
stage=$(dirname "$out")/kpkg

rm -rf "$stage"
mkdir -p "$stage/upstart" "$stage/ui"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1 | tr . ,)
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
sed "s|@VERSION@|$version|" package/manifest.json.in > "$stage/manifest.json"

# The Library tile is blank without its icon (v0.1.0 shipped like that).
grep -q '^# Icon: data:image/png;base64,[A-Za-z0-9+/]' "$scriptlet" ||
    { echo "$scriptlet has no icon" >&2; exit 1; }

cp "$bin" "$stage/kindle-wireless-sync"
cp "$scriptlet" "$stage/WirelessSync.sh"
cp package/install.sh package/uninstall.sh package/launch.sh \
   package/service.sh package/autostart.sh package/register-app.sh "$stage/"
cp package/upstart/*.conf "$stage/upstart/"
cp package/ui/* "$stage/ui/"
chmod 755 "$stage"/*.sh "$stage/kindle-wireless-sync"
chmod 644 "$stage/manifest.json" "$stage"/upstart/* "$stage"/ui/*

# KPM needs manifest.json at the archive root (no "./"), so name every entry.
# Root-owned entries, and no macOS metadata (._* files, xattrs).
if tar --version 2>/dev/null | grep -q bsdtar; then
    owner="--uid 0 --gid 0 --no-xattrs --no-acls --no-mac-metadata"
else
    owner="--owner=0 --group=0 --numeric-owner --no-xattrs --no-acls"
fi
# shellcheck disable=SC2086 # $owner is a list of flags
COPYFILE_DISABLE=1 tar $owner -czf "$out" -C "$stage" \
    manifest.json install.sh uninstall.sh launch.sh kindle-wireless-sync \
    service.sh autostart.sh register-app.sh WirelessSync.sh upstart ui

tar -tzf "$out" | grep -qx manifest.json || { echo "manifest.json is not at the archive root" >&2; exit 1; }
ls -l "$out"
