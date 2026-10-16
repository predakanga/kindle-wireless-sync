#!/bin/sh
# Developer tool: turns the Kindle's native series grouping on or off on a
# device whose marketplace isn't offered it (the AU reference device). Not
# part of the package, since the app must not change Amazon's settings.
# See docs/research/kindle-library-integration.md, "Which change does it".
#
# Runs on the Kindle; from the dev machine use `just series-grouping`.
#
# Usage: series-grouping.sh [status | enable | disable]
#
# enable:  appreg.db dcc series.grouping.enabled = true (the actual switch),
#          and this device's marketplace added to a dcc override of
#          series.grouping.supported.marketplaces (shows the "Group series"
#          switch under Settings > Home and Library > Library tab).
# disable: both back as found on the reference device: enabled = false (not
#          deleted, since the dcd default is true) and no override.
# Both apply live; the Library picks them up when it next redraws.
set -eu

APPREG=${APPREG:-/var/local/appreg.db}

die() { echo "series-grouping: $*" >&2; exit 1; }

# appreg.db is held open by running services, so wait out locks.
sql() { sqlite3 -cmd ".timeout 5000" "$APPREG" "$1"; }

prop() { sql "select value from properties where handlerId='$1' and name='$2';"; }

set_dcc() { sql "insert or replace into properties(handlerId,name,value) values('dcc','$1','$2');"; }

contains() { case ",$1," in *",$2,"*) return 0 ;; esac; return 1; }

marketplace_id() {
    id=$(prop dcc marketplace.obfuscated.id)
    [ -n "$id" ] || id=$(prop dcd marketplace.obfuscated.id)
    case $id in
        '' | *[!A-Z0-9]*) die "unexpected marketplace id '$id'" ;;
    esac
    echo "$id"
}

status() {
    id=$(marketplace_id)
    list=$(prop dcc series.grouping.supported.marketplaces)
    [ -n "$list" ] || list=$(prop dcd series.grouping.supported.marketplaces)
    enabled=$(prop dcc series.grouping.enabled)
    [ -n "$enabled" ] || enabled="$(prop dcd series.grouping.enabled) (default)"
    if contains "$list" "$id"; then shown=yes; else shown=no; fi
    echo "grouping enabled: $enabled"
    echo "settings switch:  $shown (marketplace $id)"
}

enable() {
    id=$(marketplace_id)
    defaults=$(prop dcd series.grouping.supported.marketplaces)
    contains "$defaults" "$id" ||
        set_dcc series.grouping.supported.marketplaces "${defaults:+$defaults,}$id"
    set_dcc series.grouping.enabled true
}

disable() {
    set_dcc series.grouping.enabled false
    sql "delete from properties where handlerId='dcc' and name='series.grouping.supported.marketplaces';"
}

case ${1:-status} in
    status) status ;;
    enable) enable; status ;;
    disable) disable; status ;;
    *) die "unknown command '$1' (status, enable, disable)" ;;
esac
