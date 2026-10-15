#!/bin/sh
# `kpm launch kindle-wireless-sync` opens the settings app, as the Library
# tile does (ADR 0012).
exec sh "$(dirname "$0")/WirelessSync.sh"
