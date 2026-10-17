/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Prototype only: a fake Wireless Sync service and a fake network of
 * Calibre libraries, answering the API the prototype app expects. The
 * harness (../index.html) runs one and controls it; opened on its own, the
 * app runs one in its own page. ES5, so the app can also run on the Kindle.
 *
 * The API is a proposal, not the daemon's current one: /probe and /settings
 * are new, and /link takes the probed library and its settings.
 */
(function (global) {
    "use strict";

    // Columns a fake library can have, by Calibre datatype.
    var COLUMN_CATALOG = {
        bool: [
            { lookup: "#kindle_read", name: "Kindle read" },
            { lookup: "#finished", name: "Finished" },
            { lookup: "#read", name: "Read" }
        ],
        datetime: [
            { lookup: "#kindle_last_read", name: "Kindle last read" },
            { lookup: "#date_read", name: "Date read" }
        ]
    };

    function clone(o) { return o === undefined ? undefined : JSON.parse(JSON.stringify(o)); }

    function columnsOf(lookups) {
        var out = { bool: [], datetime: [] };
        for (var kind in COLUMN_CATALOG) {
            if (!COLUMN_CATALOG.hasOwnProperty(kind)) continue;
            for (var i = 0; i < COLUMN_CATALOG[kind].length; i++) {
                var c = COLUMN_CATALOG[kind][i];
                if (lookups.indexOf(c.lookup) >= 0) out[kind].push(clone(c));
            }
        }
        return out;
    }

    function defaultNetwork() {
        return [
            { uuid: "3f6a9c2e-7d41-4b8e-9c15-2a7e0d4b6f81", name: "Calibre Library",
              hostname: "home-laptop", host: "192.168.1.50", port: 9090,
              running: true, discoverable: true, password: "", busy: false,
              columns: ["#kindle_read", "#finished", "#kindle_last_read", "#date_read"] },
            { uuid: "b2f1d0aa-6c1e-4d0b-9a55-0c3f7e9d1a20", name: "Family Library",
              hostname: "family-desktop", host: "192.168.16.20", port: 9090,
              running: true, discoverable: true, password: "kindle", busy: false,
              columns: [] },
            { uuid: "9a7e5c11-2b44-4f7f-8d0e-5e6a1b2c3d4e", name: "Work Library",
              hostname: "work-laptop", host: "10.8.0.12", port: 9090,
              running: true, discoverable: false, password: "", busy: false,
              columns: ["#read"] }
        ];
    }

    function Mock(onChange) {
        this.onChange = onChange || function () {};
        this.log = [];
        this.timers = {};
        this.variants = { firstPage: "welcome", changeFlow: "choose", replaceConfirm: "dialog", serviceControls: "none" };
        this.options = { probeMs: 2500, keyboard: true };
        this.preset("fresh");
    }

    Mock.COLUMN_CATALOG = COLUMN_CATALOG;

    Mock.PRESETS = {
        fresh: "Fresh install (nothing linked)",
        connected: "Linked and connected",
        readsync: "Linked, read sync on",
        calibre_closed: "Linked, Calibre not running",
        password_changed: "Linked, Calibre's password changed",
        no_wifi: "Linked, Wi-Fi off"
    };

    Mock.prototype.preset = function (name) {
        for (var t in this.timers) if (this.timers.hasOwnProperty(t)) clearTimeout(this.timers[t]);
        this.timers = {};
        this.net = defaultNetwork();
        this.wifi = true;
        this.svc = { running: true, autostart: name !== "fresh", series: true, paused: false, linked: null,
                     state: "not_linked", detail: "" };
        this.found = [];
        this.finding = false;
        this.probe = null;
        if (name !== "fresh") {
            var lib = this.net[0];
            this.svc.linked = {
                uuid: lib.uuid, name: lib.name, hostname: lib.hostname, host: lib.host, port: lib.port,
                password: "", writable: name === "readsync",
                read_column: name === "readsync" ? "#kindle_read" : null,
                last_read_column: name === "readsync" ? "#kindle_last_read" : null,
                columns: columnsOf(lib.columns)
            };
            if (name === "calibre_closed") lib.running = false;
            if (name === "password_changed") lib.password = "newpass";
            if (name === "no_wifi") this.wifi = false;
            this.svc.state = "connected";
        }
        this.reconcile(true);
        this.note("preset: " + name);
    };

    Mock.prototype.note = function (line) {
        this.log.unshift(line);
        if (this.log.length > 60) this.log.length = 60;
        this.onChange();
    };

    Mock.prototype.later = function (key, ms, fn) {
        var self = this;
        if (this.timers[key]) clearTimeout(this.timers[key]);
        this.timers[key] = setTimeout(function () { delete self.timers[key]; fn.call(self); self.onChange(); }, ms);
    };

    Mock.prototype.libByUuid = function (uuid) {
        for (var i = 0; i < this.net.length; i++) if (this.net[i].uuid === uuid) return this.net[i];
        return null;
    };

    Mock.prototype.libByAddress = function (host, port) {
        for (var i = 0; i < this.net.length; i++) {
            var l = this.net[i];
            if ((l.host === host || l.hostname.toLowerCase() === String(host).toLowerCase()) && l.port === port) return l;
        }
        return null;
    };

    /// What the daemon would be doing now. `instant` skips the brief
    /// "searching" step (presets start in their final state).
    Mock.prototype.reconcile = function (instant) {
        var s = this.svc, linked = s.linked;
        var target, detail;
        if (!linked) { target = "not_linked"; detail = "No Calibre library is linked yet."; }
        else if (s.paused) { target = "paused"; detail = "Paused. Resume to connect to " + linked.name + "."; }
        else if (!this.wifi) { target = "waiting_for_wifi"; detail = "Waiting for Wi-Fi."; }
        else {
            var lib = this.libByUuid(linked.uuid);
            if (!lib || !lib.running) {
                target = "not_found"; detail = linked.name + " not found. Retrying in 60s.";
            } else if (lib.password && lib.password !== linked.password) {
                target = "password_rejected"; detail = linked.name + " rejected the password.";
            } else {
                target = "connected"; detail = "Connected to " + linked.name + ".";
                linked.columns = columnsOf(lib.columns);
            }
        }
        if (target === "connected" && s.state !== "connected" && !instant) {
            s.state = "searching";
            s.detail = "Looking for " + linked.name + ".";
            this.later("connect", 1500, function () { this.reconcile(true); });
            return;
        }
        s.state = target;
        s.detail = detail;
    };

    Mock.prototype.status = function () {
        var s = this.svc, l = s.linked;
        return {
            ok: true, version: "0.3.0", state: s.state, detail: s.detail,
            paused: s.paused, autostart: s.autostart, series: s.series,
            library: l ? {
                uuid: l.uuid, name: l.name, hostname: l.hostname, host: l.host, port: l.port,
                has_password: !!l.password, writable: l.writable,
                read_column: l.read_column, last_read_column: l.last_read_column,
                columns: clone(l.columns)
            } : null,
            found: clone(this.found), finding: this.finding,
            probe: clone(this.probe),
            prototype: { variants: clone(this.variants), keyboard: this.options.keyboard }
        };
    };

    /// The API. cb(data, err) like the app's XHR wrapper.
    Mock.prototype.request = function (method, path, body, cb) {
        var self = this;
        if (path !== "/status") this.note(method + " " + path + (body && JSON.stringify(body) !== "{}" ? " " + JSON.stringify(body) : ""));
        setTimeout(function () {
            if (!self.svc.running) return cb(null, "The service isn't running.");
            var out;
            try { out = self.handle(method, path, body || {}); }
            catch (e) { return cb(null, String(e)); }
            // Reading the status changes nothing; reporting it as a change
            // made the harness poke the app into polling again, in a loop.
            if (path !== "/status") self.onChange();
            cb(out, null);
        }, 120);
    };

    Mock.prototype.handle = function (method, path, b) {
        var s = this.svc;
        if (path === "/status") return this.status();
        if (path === "/find") {
            this.finding = true;
            this.later("find", 2000, function () {
                this.finding = false;
                this.found = [];
                for (var i = 0; i < this.net.length; i++) {
                    var l = this.net[i];
                    if (!this.wifi || !l.running || !l.discoverable) continue;
                    if (s.linked && s.linked.uuid === l.uuid) continue;
                    this.found.push({ uuid: l.uuid, name: l.name, hostname: l.hostname, host: l.host, port: l.port });
                }
            });
            return { ok: true };
        }
        if (path === "/probe") return this.startProbe(b);
        if (path === "/probe/cancel") { this.probe = null; return { ok: true }; }
        if (path === "/link") return this.link(b);
        if (path === "/settings") {
            if (!s.linked) return { ok: false, error: "No library is linked." };
            s.linked.writable = !!b.writable;
            s.linked.read_column = b.read_column || null;
            s.linked.last_read_column = b.last_read_column || null;
            if (s.state === "connected") {
                // ADR 0017: the session ends at the next quiet keep-alive and reconnects.
                s.state = "searching";
                s.detail = "Reconnecting to use the new settings.";
                this.later("connect", 2500, function () { this.reconcile(true); });
            }
            return { ok: true };
        }
        if (path === "/password") {
            if (!s.linked) return { ok: false, error: "No library is linked." };
            s.linked.password = b.password || "";
            this.reconcile();
            return { ok: true };
        }
        if (path === "/unlink") { s.linked = null; this.reconcile(); return { ok: true }; }
        if (path === "/autostart") { s.autostart = !!b.enabled; return { ok: true }; }
        if (path === "/series") { s.series = !!b.enabled; return { ok: true }; }
        if (path === "/pause") { s.paused = true; this.reconcile(); return { ok: true }; }
        if (path === "/resume") { s.paused = false; this.reconcile(); return { ok: true }; }
        if (path === "/connect-now") { this.reconcile(); return { ok: true }; }
        if (path === "/service") {
            if (b.action === "stop") {
                this.later("stop", 300, function () { s.running = false; });
            } else {
                s.state = "searching"; s.detail = "Restarting.";
                this.later("connect", 2000, function () { this.reconcile(true); });
            }
            return { ok: true };
        }
        return { ok: false, error: "No such API call: " + path };
    };

    /// Connect to a library, check the password and read its columns, then
    /// disconnect (the proposal). The result appears in /status as `probe`.
    Mock.prototype.startProbe = function (b) {
        var lib = b.uuid ? this.libByUuid(b.uuid) : this.libByAddress(b.host, parseInt(b.port, 10));
        var found = null;
        for (var i = 0; b.uuid && i < this.found.length; i++) if (this.found[i].uuid === b.uuid) found = this.found[i];
        var label = lib && (b.uuid || lib.discoverable) ? lib.name : (b.host + ":" + b.port);
        if (found) label = found.name;
        this.probe = { state: "checking", target: { uuid: b.uuid || null, host: b.host || (found && found.host),
                       port: b.port || (found && found.port), name: label }, password: b.password || "" };
        var probe = this.probe;
        this.later("probe", this.options.probeMs, function () {
            if (this.probe !== probe) return; // cancelled or replaced
            if (!this.wifi) { probe.state = "unreachable"; probe.error = "The Kindle isn't on Wi-Fi."; return; }
            if (!lib || !lib.running) {
                probe.state = "unreachable";
                probe.error = "Nothing answered at " + (probe.target.host || "?") + ":" + (probe.target.port || "?")
                    + ". Check that Calibre's wireless device connection is started.";
                return;
            }
            if (lib.busy) { probe.state = "busy"; probe.error = "Calibre is busy with another device."; return; }
            if (lib.password && !b.password) { probe.state = "password_needed"; return; }
            if (lib.password && b.password !== lib.password) { probe.state = "password_rejected"; return; }
            probe.state = "ok";
            probe.library = { uuid: lib.uuid, name: lib.name, hostname: lib.hostname, host: lib.host,
                              port: lib.port, columns: columnsOf(lib.columns) };
        });
        return { ok: true };
    };

    Mock.prototype.link = function (b) {
        var s = this.svc, p = this.probe;
        if (!p || p.state !== "ok") return { ok: false, error: "Check the library first." };
        var lib = p.library;
        if (s.linked && s.linked.uuid !== lib.uuid && !b.replace) {
            return { ok: false, needs_replace: true, error: "Another library is linked.",
                     linked: { uuid: s.linked.uuid, name: s.linked.name },
                     library: { uuid: lib.uuid, name: lib.name } };
        }
        var replaced = !!s.linked && s.linked.uuid !== lib.uuid;
        s.linked = { uuid: lib.uuid, name: lib.name, hostname: lib.hostname, host: lib.host, port: lib.port,
                     password: p.password || "", writable: !!b.writable,
                     read_column: b.read_column || null, last_read_column: b.last_read_column || null,
                     columns: clone(lib.columns) };
        this.probe = null;
        this.found = [];
        s.paused = false;
        s.state = "not_found";
        this.reconcile();
        if (replaced) s.detail = "Resetting the Kindle's book details, then connecting to " + lib.name + ".";
        return { ok: true, replaced: replaced };
    };

    global.KWSMock = Mock;
})(this);
