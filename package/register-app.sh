#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Registers or unregisters the Wireless Sync WAF app in appreg.db, so
# `lipc-set-prop com.lab126.appmgrd start app://com.kindlewirelesssync.app`
# opens it in mesquite (ADR 0004). Only our own handler's rows are touched.
APP_ID=com.kindlewirelesssync.app
APP_DIR=/var/local/kindle-wireless-sync/ui
DB=/var/local/appreg.db

case "$1" in
register)
    sqlite3 "$DB" <<SQL
INSERT OR IGNORE INTO interfaces (interface) VALUES ('application');
INSERT OR IGNORE INTO handlerIds (handlerId) VALUES ('$APP_ID');
INSERT OR IGNORE INTO associations (handlerId, interface, contentId, defaultAssoc)
    VALUES ('$APP_ID', 'application', 'GL:$APP_ID', 0);
INSERT OR REPLACE INTO properties (handlerId, name, value) VALUES ('$APP_ID', 'lipcId', '$APP_ID');
INSERT OR REPLACE INTO properties (handlerId, name, value)
    VALUES ('$APP_ID', 'command', '/usr/bin/mesquite -l $APP_ID -c file://$APP_DIR/');
INSERT OR REPLACE INTO properties (handlerId, name, value) VALUES ('$APP_ID', 'supportedOrientation', 'U');
SQL
    ;;
unregister)
    sqlite3 "$DB" <<SQL
DELETE FROM properties WHERE handlerId = '$APP_ID';
DELETE FROM associations WHERE handlerId = '$APP_ID';
DELETE FROM handlerIds WHERE handlerId = '$APP_ID';
SQL
    rm -rf "/var/local/mesquite/$APP_ID"
    ;;
*)
    echo "usage: $0 register|unregister" >&2
    exit 2
    ;;
esac
