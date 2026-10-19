/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Page-fit harness for the Wireless Sync app (tools/compat). compat.py
 * copies package/ui into a separate test app and loads this file before the
 * real script.js. It replaces XMLHttpRequest with a fake service, so the
 * real app draws each page from fixture data, then walks the app to one page
 * per scenario, measures it and reports to the host through an SSH tunnel
 * (127.0.0.1:COLLECTOR_PORT on the Kindle). The host takes a screenshot
 * and answers; the page then reloads for the next scenario.
 *
 * ES5 only, like the app: mesquite has no promises or arrow functions.
 */
(function () {
    "use strict";

    var COLLECTOR = "http://127.0.0.1:18326";
    var STEP_MS = 500;      // after each step, for the app to redraw
    var SETTLE_MS = 800;    // after the app's first draw
    var KEYBOARD_MS = 2500; // after focusing a field, for the keyboard to open

    var RealXHR = window.XMLHttpRequest;

    // ---- talking to the host ----

    function send(path, body, sync) {
        var xhr = new RealXHR();
        xhr.open(body === null ? "GET" : "POST", COLLECTOR + path, !sync);
        if (body !== null) xhr.setRequestHeader("Content-Type", "text/plain");
        xhr.send(body === null ? null : JSON.stringify(body));
        if (sync) return xhr.status === 200 ? JSON.parse(xhr.responseText) : null;
        return xhr;
    }

    function sendThen(path, body, cb) {
        var xhr = new RealXHR();
        xhr.onreadystatechange = function () { if (xhr.readyState === 4) cb(xhr.status); };
        xhr.open("POST", COLLECTOR + path, true);
        xhr.setRequestHeader("Content-Type", "text/plain");
        xhr.send(JSON.stringify(body));
    }

    // ---- fixtures ----

    function clone(v) { return v === undefined ? undefined : JSON.parse(JSON.stringify(v)); }

    function merge(base, over) {
        var out = clone(base);
        for (var k in over) if (over.hasOwnProperty(k)) out[k] = clone(over[k]);
        return out;
    }

    var NOW = Math.floor(new Date().getTime() / 1000);

    // Long names everywhere: the worst case for wrapping.
    var LIB_NAME = "Science Fiction and Fantasy Collection (Main Library)";
    var OTHER_NAME = "Non-fiction, Reference and Technical Manuals";

    var MANY_COLUMNS = {
        "#kindle_read": { name: "Kindle read", datatype: "bool" },
        "#read": { name: "Read", datatype: "bool" },
        "#finished_reading": { name: "Finished reading (any device)", datatype: "bool" },
        "#owned_in_print": { name: "Owned in print", datatype: "bool" },
        "#to_review": { name: "Needs a review written", datatype: "bool" },
        "#lent_out": { name: "Lent to a friend", datatype: "bool" },
        "#kindle_last_read": { name: "Kindle last read", datatype: "datetime" },
        "#date_read": { name: "Date read", datatype: "datetime" },
        "#date_started": { name: "Date started reading", datatype: "datetime" },
        "#purchased": { name: "Date purchased", datatype: "datetime" },
        "#reviewed_on": { name: "Date reviewed", datatype: "datetime" },
        "#lent_on": { name: "Lent on", datatype: "datetime" }
    };

    var LIB = {
        uuid: "7c2e1f0a-4b5d-4e6f-8a9b-0c1d2e3f4a5b", name: LIB_NAME, host: "192.168.100.200", port: 9090,
        hostname: "family-room-macbook-pro.local", has_password: true, writable: true,
        read_column: "#kindle_read", last_read_column: "#kindle_last_read", columns: MANY_COLUMNS
    };

    var FOUND = [];
    (function () {
        var names = [OTHER_NAME, "Comics and Graphic Novels Archive", "Children's Picture Books",
                     "Calibre Library", "Work: Standards and Specifications"];
        for (var i = 0; i < names.length; i++) {
            FOUND.push({ uuid: "0000000" + i + "-1111-2222-3333-444444444444", name: names[i],
                         host: "192.168.100." + (201 + i), port: 9090 + i,
                         hostname: "computer-number-" + (i + 1) + "-in-the-house.local" });
        }
    })();

    var BASE = {
        ok: true, version: "0.4.0", state: "connected", detail: "", since: NOW - 600, retry_at: null,
        advice: null, paused: false, autostart: true, autostart_disabled_by_guard: false, series: true,
        books_folder: "Calibre/Science Fiction", wifi: true, probe: null, library: LIB, found: []
    };

    var UNLINKED = merge(BASE, { library: null, state: "not_linked" });

    var FOLDERS = [];
    (function () {
        for (var i = 1; i <= 14; i++) {
            FOLDERS.push({ name: "Folder number " + i + " with a fairly long name", problem: null });
        }
        FOLDERS[3].problem = "A folder name can't start with a dot.";
    })();

    function probeAt(state, extra) {
        var p = { state: state, target: { name: OTHER_NAME, host: FOUND[0].host, port: FOUND[0].port,
                                          uuid: FOUND[0].uuid },
                  error: null, library: null };
        return merge(p, extra || {});
    }

    var CHECKED = { uuid: FOUND[0].uuid, name: OTHER_NAME, host: FOUND[0].host, port: FOUND[0].port,
                    hostname: FOUND[0].hostname, columns: MANY_COLUMNS };

    var LOCKED_OUT = "Calibre may have stopped accepting this Kindle after too many failed attempts. "
        + "In Calibre, stop and start the wireless device connection (Connect/share), then try again.";

    // Each scenario: the service's state, the steps that reach the page, and
    // text the page must show (to be sure the steps got there).
    // Steps: ["click", act, arg?], ["focus", inputId], ["status", {patch}].
    var SCENARIOS = [
        { name: "home", status: BASE, expect: "Linked to" },
        { name: "home-not-found", status: merge(BASE, { state: "not_found", retry_at: NOW + 3600,
            advice: "Calibre at 192.168.100.200 is serving " + OTHER_NAME + ", not " + LIB_NAME
                + ". Switch back to " + LIB_NAME + " in Calibre, then use Connect." }),
          expect: "Library not available" },
        { name: "home-ejected", status: merge(BASE, { state: "ejected",
            advice: "Ejected by Calibre.\nTap Connect below to reconnect." }), expect: "Ejected" },
        { name: "home-password-rejected", status: merge(BASE, { state: "password_rejected" }),
          expect: "Enter the password" },
        { name: "home-locked-out", status: merge(BASE, { state: "locked_out", advice: LOCKED_OUT }),
          expect: "isn't accepting" },
        { name: "home-error", status: merge(BASE, { state: "error",
            detail: "Couldn't write to /mnt/us/documents/Calibre/Science Fiction: No space left on device (os error 28)" }),
          expect: "Problem" },
        { name: "dialog-forget", status: BASE, steps: [["click", "unlink"]], expect: "Forget " + LIB_NAME + "?",
          dialog: true },
        { name: "settings", status: BASE, steps: [["click", "go", "settings"]], expect: "Computer" },
        { name: "settings-picker-read", status: BASE,
          steps: [["click", "go", "settings"], ["click", "picker", "bool"]], expect: "Read column" },
        { name: "settings-picker-last-read", status: BASE,
          steps: [["click", "go", "settings"], ["click", "picker", "datetime"]], expect: "Last-read column" },
        { name: "settings-picker-no-columns",
          status: merge(BASE, { library: merge(LIB, { columns: {} }) }),
          steps: [["click", "go", "settings"], ["click", "picker", "bool"]], expect: "has no Yes/No columns" },
        { name: "password", status: BASE, steps: [["click", "go", "settings"], ["click", "password", "settings"]],
          expect: "as set in Calibre" },
        { name: "password-keyboard", status: BASE,
          steps: [["click", "go", "settings"], ["click", "password", "settings"], ["focus", "inPw"]],
          expect: "as set in Calibre", keyboard: true },
        { name: "app-settings", status: merge(BASE, { autostart: false, autostart_disabled_by_guard: true }),
          steps: [["click", "appsettings"]], expect: "Autostart was turned off" },
        { name: "folders", status: BASE, folders: { path: "Calibre/Science Fiction/Very long subfolder name", list: FOLDERS },
          steps: [["click", "appsettings"], ["click", "folders"]], expect: "Current folder" },
        { name: "new-folder", status: BASE, folders: { path: "Calibre/Science Fiction/Very long subfolder name", list: FOLDERS },
          steps: [["click", "appsettings"], ["click", "folders"], ["click", "go", "newfolder"]],
          expect: "A new folder inside" },
        { name: "new-folder-keyboard", status: BASE, folders: { path: "Calibre", list: FOLDERS },
          steps: [["click", "appsettings"], ["click", "folders"], ["click", "go", "newfolder"], ["focus", "inFolder"]],
          expect: "A new folder inside", keyboard: true },
        { name: "choose-empty", status: UNLINKED, servers: [], expect: "Calibre not found" },
        { name: "choose-searching", status: UNLINKED, findPending: true, expect: "Looking for Calibre" },
        { name: "choose-wifi-off", status: merge(UNLINKED, { wifi: false }), expect: "Wi-Fi is off" },
        { name: "choose-many", status: merge(UNLINKED, { found: FOUND }),
          servers: [{ hostname: "upstairs-office-pc.local", status: "error",
                      error: "Calibre isn't accepting connections from this Kindle." }],
          expect: FOUND[4].name },
        { name: "choose-linked", status: merge(BASE, { found: FOUND }), steps: [["click", "change"]],
          expect: "Linking another library replaces" },
        { name: "address", status: UNLINKED, steps: [["click", "go", "address"]], expect: "Computer name or address" },
        { name: "address-keyboard", status: UNLINKED, steps: [["click", "go", "address"], ["focus", "inHost"]],
          expect: "Computer name or address", keyboard: true },
        { name: "check-running", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("checking"),
          steps: [["click", "probe", FOUND[0].uuid]], expect: "Testing connection" },
        { name: "check-password-needed", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("password_needed"),
          steps: [["click", "probe", FOUND[0].uuid]], expect: "Password needed" },
        { name: "check-password-keyboard", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("password_rejected"),
          steps: [["click", "probe", FOUND[0].uuid], ["focus", "inProbePw"]], expect: "Wrong password", keyboard: true },
        { name: "check-failed", status: merge(UNLINKED, { found: FOUND }),
          probe: probeAt("error", { error: "Calibre closed the connection before the check finished. "
              + "It may be busy with another device, or its wireless connection may have stopped. " + LOCKED_OUT }),
          steps: [["click", "probe", FOUND[0].uuid]], expect: "Couldn't connect" },
        { name: "setup", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("ok", { library: CHECKED }),
          steps: [["click", "probe", FOUND[0].uuid]], expect: "Update read status" },
        { name: "setup-writable", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("ok", { library: CHECKED }),
          steps: [["click", "probe", FOUND[0].uuid], ["click", "writable"]], expect: "Last read" },
        { name: "setup-picker-read", status: merge(UNLINKED, { found: FOUND }), probe: probeAt("ok", { library: CHECKED }),
          steps: [["click", "probe", FOUND[0].uuid], ["click", "writable"], ["click", "picker", "bool"]],
          expect: "Refresh" },
        { name: "setup-picker-no-columns", status: merge(UNLINKED, { found: FOUND }),
          probe: probeAt("ok", { library: merge(CHECKED, { columns: {} }) }),
          steps: [["click", "probe", FOUND[0].uuid], ["click", "writable"], ["click", "picker", "datetime"]],
          expect: "has no Date columns" },
        { name: "dialog-replace", status: merge(BASE, { found: FOUND }), probe: probeAt("ok", { library: CHECKED }),
          link: { ok: false, needs_replace: true, linked: { uuid: LIB.uuid, name: LIB_NAME },
                  library: { uuid: CHECKED.uuid, name: OTHER_NAME } },
          steps: [["click", "change"], ["click", "probe", FOUND[0].uuid], ["click", "link"]],
          expect: "Replace " + LIB_NAME + "?", dialog: true },
        { name: "service-stopped", status: BASE, down: true, expect: "Service not running" }
    ];

    // ---- the fake service ----

    var S = null; // this scenario's service state

    function service(method, path, body) {
        if (S.down) return null;
        var ok = { ok: true };
        switch (path) {
        case "/status": return S.status;
        case "/find":
            if (S.findPending) return "pending";
            if (S.servers) return { ok: true, servers: S.servers };
            // By default every found library's computer answered.
            var servers = [], found = S.status.found || [];
            for (var i = 0; i < found.length; i++) servers.push({ hostname: found[i].hostname, status: "ok" });
            return { ok: true, servers: servers };
        case "/probe": S.status.probe = clone(S.probe) || null; return ok;
        case "/probe/cancel": S.status.probe = null; return ok;
        case "/folders":
            var f = S.folders || { path: "", list: [] };
            return { ok: true, path: f.path, folders: f.list };
        case "/link": return S.link || ok;
        default: return ok;
        }
    }

    function FakeXHR() { this.readyState = 0; this.status = 0; this.responseText = ""; }
    FakeXHR.prototype.open = function (method, url) { this.method = method; this.url = url; };
    FakeXHR.prototype.setRequestHeader = function () {};
    FakeXHR.prototype.abort = function () { this.aborted = true; };
    FakeXHR.prototype.send = function (body) {
        var self = this;
        var path = String(self.url).replace(/^[a-z]+:\/\/[^\/]+/, "");
        var reply = service(self.method, path, body ? JSON.parse(body) : null);
        if (reply === "pending") return;
        setTimeout(function () {
            if (self.aborted) return;
            if (reply === null) {
                self.status = 0;
            } else {
                self.status = 200;
                self.responseText = JSON.stringify(reply);
            }
            self.readyState = 4;
            if (self.onreadystatechange) self.onreadystatechange();
        }, 0);
    };

    // The app polls /status every 3 s. Hold on to the poll so a status
    // change shows at once, and don't let it run by itself: pages stay put.
    var poll = null;
    var realSetInterval = window.setInterval;
    window.setInterval = function (fn, ms) {
        if (ms === 3000) { poll = fn; return 0; }
        return realSetInterval(fn, ms);
    };

    // ---- running a scenario ----

    var index = -1;
    var scenario = null;
    var finished = false;

    function finish(result) {
        if (finished) return;
        finished = true;
        result.index = index;
        result.name = scenario ? scenario.name : null;
        // The host takes a screenshot before it answers.
        sendThen("/result", result, function () { window.location.reload(); });
    }

    window.onerror = function (msg, src, line) {
        finish({ error: "script error: " + msg + " (" + src + ":" + line + ")" });
    };

    function click(act, arg) {
        var all = document.querySelectorAll("[data-act]");
        for (var i = 0; i < all.length; i++) {
            var e = all[i];
            if (e.getAttribute("data-act") !== act) continue;
            if (arg !== undefined && e.getAttribute("data-arg") !== String(arg)) continue;
            var ev = document.createEvent("MouseEvents");
            ev.initMouseEvent("click", true, true, window, 1, 0, 0, 0, 0, false, false, false, false, 0, null);
            e.dispatchEvent(ev);
            return null;
        }
        return "nothing to tap for " + act + (arg !== undefined ? " " + arg : "");
    }

    function describe(e) {
        var cls = e.className && typeof e.className === "string" ? "." + e.className.replace(/\s+/g, ".") : "";
        var text = (e.textContent || "").replace(/\s+/g, " ").substring(0, 40);
        return e.tagName.toLowerCase() + cls + (text ? " \"" + text + "\"" : "");
    }

    function measure() {
        var pageEl = document.getElementById("page");
        var doc = document.documentElement;
        var w = window.innerWidth, h = window.innerHeight;
        var m = {
            inner_width: w, inner_height: h,
            used_height: pageEl.offsetTop + pageEl.offsetHeight,
            scroll_height: Math.max(doc.scrollHeight, document.body.scrollHeight),
            scroll_width: Math.max(doc.scrollWidth, document.body.scrollWidth),
            root_font_px: parseFloat(window.getComputedStyle(doc, null).fontSize)
        };
        m.overflow = m.used_height - h;
        m.h_overflow = m.scroll_width - w;
        var dlg = document.querySelector("#dialog .dialog");
        if (dlg && dlg.offsetHeight) {
            var r = dlg.getBoundingClientRect();
            m.dialog_bottom = Math.round(r.bottom);
            m.dialog_overflow = Math.round(r.bottom) - h;
        }
        // Anything that pokes out to the right (cut off on the device).
        var wide = [];
        var all = document.body.getElementsByTagName("*");
        for (var i = 0; i < all.length && wide.length < 5; i++) {
            var b = all[i].getBoundingClientRect();
            if (b.width > 0 && b.right > w + 1) wide.push(describe(all[i]) + " right=" + Math.round(b.right));
        }
        m.too_wide = wide;
        // The lowest visible element on the page, for reading the overflow.
        var lowest = null, lowestBottom = 0;
        var inPage = pageEl.getElementsByTagName("*");
        for (var j = 0; j < inPage.length; j++) {
            var rb = inPage[j].getBoundingClientRect();
            if (rb.height > 0 && rb.bottom > lowestBottom) { lowestBottom = rb.bottom; lowest = inPage[j]; }
        }
        if (lowest) m.lowest = describe(lowest) + " bottom=" + Math.round(lowestBottom);
        var active = document.activeElement;
        m.focused = active && active.tagName === "INPUT" ? active.id : null;
        // The host finds the keyboard in the screenshot; the field being
        // typed in must stay above it.
        if (m.focused) m.focused_bottom = Math.round(active.getBoundingClientRect().bottom);
        return m;
    }

    function runSteps(steps, i) {
        if (i >= steps.length) {
            var text = document.body.textContent || "";
            var result = { metrics: measure(), reached: text.indexOf(scenario.expect) >= 0,
                           keyboard: !!scenario.keyboard, dialog: !!scenario.dialog };
            if (!result.reached) result.error = "page doesn't show \"" + scenario.expect + "\"";
            finish(result);
            return;
        }
        var step = steps[i];
        var wait = STEP_MS;
        var problem = null;
        if (step[0] === "click") {
            problem = click(step[1], step[2]);
        } else if (step[0] === "focus") {
            var input = document.getElementById(step[1]);
            if (input) {
                input.focus();
                wait = KEYBOARD_MS;
            } else {
                problem = "no field " + step[1];
            }
        } else if (step[0] === "status") {
            S.status = merge(S.status, step[1]);
            if (poll) poll();
        }
        if (problem) { finish({ error: "step " + (i + 1) + ": " + problem }); return; }
        setTimeout(function () { runSteps(steps, i + 1); }, wait);
    }

    /// What this browser offers, reported once per run.
    function environment() {
        var probe = document.createElement("div");
        probe.style.display = "flex";
        var k = window.kindle;
        var kindleApis = [];
        if (k) for (var name in k) kindleApis.push(name);
        return {
            user_agent: navigator.userAgent,
            inner_width: window.innerWidth, inner_height: window.innerHeight,
            screen_width: window.screen.width, screen_height: window.screen.height,
            device_pixel_ratio: window.devicePixelRatio || null,
            kindle_apis: kindleApis.sort(),
            has: {
                json: typeof JSON === "object", query_selector_all: !!document.querySelectorAll,
                add_event_listener: !!document.addEventListener, function_bind: !!Function.prototype.bind,
                array_for_each: !!Array.prototype.forEach, promise: typeof window.Promise === "function",
                fetch: typeof window.fetch === "function", flex_style: probe.style.display === "flex",
                svg: !!document.createElementNS &&
                    !!document.createElementNS("http://www.w3.org/2000/svg", "svg").createSVGRect
            }
        };
    }

    function start() {
        var state = send("/state", null, true);
        if (!state) return; // no host: leave the fake service idle
        index = state.index;
        if (index >= SCENARIOS.length) {
            S = { status: BASE, down: true };
            send("/done", { count: SCENARIOS.length }, false);
            return;
        }
        scenario = SCENARIOS[index];
        S = { status: clone(scenario.status), down: scenario.down, findPending: scenario.findPending,
              servers: scenario.servers, probe: scenario.probe, folders: scenario.folders, link: scenario.link };
        window.addEventListener("load", function () {
            var run = function () { setTimeout(function () { runSteps(scenario.steps || [], 0); }, SETTLE_MS); };
            if (index !== 0) { run(); return; }
            // Once per run: what the browser offers.
            var env = environment();
            env.scenarios = [];
            for (var i = 0; i < SCENARIOS.length; i++) env.scenarios.push(SCENARIOS[i].name);
            sendThen("/env", env, run);
        }, false);
    }

    window.XMLHttpRequest = FakeXHR;
    start();
})();
