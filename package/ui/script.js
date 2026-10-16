/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Wireless Sync settings app. Talks to the daemon's localhost API
 * (ADR 0011). ES5 only: the Kindle's mesquite engine has no fetch, promises
 * or arrow functions.
 */
(function () {
    "use strict";

    var API = "http://127.0.0.1:8325";
    var POLL_MS = 3000;
    var MESSAGE_MS = 6000;

    var lastStatusText = null;
    var status = null;
    var busy = false;          // a slow action (find, add) is running
    var confirmForget = null;  // uuid awaiting a second tap on Forget
    var editingPassword = null; // uuid whose password form is open
    var messageTimer = null;

    // ---- HTTP ----

    function request(method, path, body, cb) {
        var xhr = new XMLHttpRequest();
        var done = false;
        var timer = setTimeout(function () {
            if (!done) { done = true; xhr.abort(); cb(null, "The service didn't answer."); }
        }, 30000);
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4 || done) return;
            done = true;
            clearTimeout(timer);
            if (xhr.status === 0) return cb(null, "The service isn't running.");
            var data = null;
            try { data = JSON.parse(xhr.responseText); } catch (e) { /* handled below */ }
            if (!data) return cb(null, "Unexpected reply (HTTP " + xhr.status + ").");
            if (xhr.status !== 200) return cb(null, data.error || ("HTTP " + xhr.status));
            cb(data, null);
        };
        try {
            xhr.open(method, API + path, true);
            if (body !== null) {
                // text/plain keeps this a "simple" request (no CORS preflight).
                xhr.setRequestHeader("Content-Type", "text/plain");
                xhr.send(JSON.stringify(body));
            } else {
                xhr.send(null);
            }
        } catch (e) {
            done = true;
            clearTimeout(timer);
            cb(null, "Couldn't reach the service.");
        }
    }

    /// POST and report user-level failures ({ok: false}) as messages.
    function act(path, body, okText, after) {
        request("POST", path, body || {}, function (data, err) {
            if (err) { showMessage(err); }
            else if (!data.ok) { showMessage(data.error || "That didn't work."); }
            else if (okText) { showMessage(okText); }
            if (after) after(data, err);
            refresh(true);
        });
    }

    // ---- helpers ----

    function el(id) { return document.getElementById(id); }

    function esc(s) {
        if (s === null || s === undefined) return "";
        return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;")
            .replace(/>/g, "&gt;").replace(/"/g, "&quot;");
    }

    function show(id, visible) {
        var e = el(id);
        e.className = e.className.replace(/\s*hidden/g, "") + (visible ? "" : " hidden");
    }

    function showMessage(text) {
        var m = el("message");
        m.innerHTML = esc(text);
        m.className = "message visible";
        if (messageTimer) clearTimeout(messageTimer);
        messageTimer = setTimeout(function () { m.className = "message"; }, MESSAGE_MS);
    }

    function setBusy(b, label) {
        busy = b;
        el("btnFind").disabled = b;
        el("btnAdd").disabled = b;
        if (label) el(label.id).innerHTML = esc(b ? label.busy : label.idle);
    }

    var STATE_TITLES = {
        waiting_for_wifi: "Waiting for Wi-Fi",
        no_libraries: "No library set up",
        searching: "Looking for Calibre",
        not_found: "Not connected",
        connected: "Connected",
        ejected: "Disconnected by Calibre",
        password_rejected: "Password rejected",
        error: "Problem",
        paused: "Paused",
        stopped: "Stopped"
    };

    function address(lib) {
        if (!lib.host) return "address not known yet";
        return lib.host + (lib.port ? ":" + lib.port : "");
    }

    // ---- rendering ----

    function renderStatus(s) {
        el("statusState").innerHTML = esc(STATE_TITLES[s.state] || s.state);
        el("statusDetail").innerHTML = esc(s.detail || "");
        var canConnect = !s.paused && s.state !== "connected" && s.state !== "searching"
            && s.state !== "waiting_for_wifi" && s.state !== "no_libraries";
        el("btnConnect").disabled = !canConnect;
        el("btnPause").innerHTML = s.paused ? "Resume" : "Pause";
        el("chkAutostart").checked = !!s.autostart;
        show("guardWarning", !!s.autostart_disabled_by_guard && !s.autostart);
        if (s.config_error) showMessage("Settings file problem: " + s.config_error);
    }

    function renderAllowed(list) {
        if (!list || !list.length) {
            el("allowedList").innerHTML = '<div class="empty">No libraries yet. Use Find libraries below.</div>';
            return;
        }
        var html = "";
        for (var i = 0; i < list.length; i++) {
            var a = list[i];
            var u = esc(a.uuid);
            html += '<div class="item">'
                + '<div class="item-head"><span class="item-name">' + esc(a.name || a.uuid) + '</span>'
                + '<button class="btn btn-small" data-act="password" data-uuid="' + u + '">Password</button>'
                + '<button class="btn btn-small' + (confirmForget === a.uuid ? ' btn-danger' : '') + '" data-act="forget" data-uuid="' + u + '">'
                + (confirmForget === a.uuid ? 'Tap to confirm' : 'Forget') + '</button></div>'
                + '<div class="item-sub">' + esc(address(a)) + (a.has_password ? " · password set" : "") + '</div>';
            if (editingPassword === a.uuid) {
                html += '<div class="item-form">'
                    + '<input type="password" id="pw-' + u + '" placeholder="Calibre password (empty to clear)" />'
                    + '<div class="row" style="margin-top:0.5rem">'
                    + '<button class="btn" data-act="savepw" data-uuid="' + u + '">Save</button>'
                    + '<button class="btn" data-act="cancelpw">Cancel</button></div></div>';
            }
            html += '</div>';
        }
        el("allowedList").innerHTML = html;
    }

    function renderSeen(list) {
        show("seenHeading", !!(list && list.length));
        var html = "";
        for (var i = 0; list && i < list.length; i++) {
            var s = list[i];
            html += '<div class="item"><div class="item-head"><span class="item-name">' + esc(s.name || s.uuid) + '</span>'
                + '<button class="btn btn-small" data-act="allow" data-uuid="' + esc(s.uuid) + '">Allow</button></div>'
                + '<div class="item-sub">' + esc((s.hostname ? s.hostname + " · " : "") + address(s)) + '</div></div>';
        }
        el("seenList").innerHTML = html;
    }

    function render(s) {
        status = s;
        renderStatus(s);
        // Don't redraw lists under an open password form: it would eat input.
        if (!editingPassword) {
            renderAllowed(s.allowed);
            renderSeen(s.seen);
        }
    }

    // ---- polling ----

    function refresh(force) {
        request("GET", "/status", null, function (data, err) {
            if (err) {
                el("statusState").innerHTML = "Service not running";
                el("statusDetail").innerHTML = esc(err + " Close and reopen Wireless Sync to start it.");
                lastStatusText = null;
                return;
            }
            // E-ink: only redraw when something changed.
            var text = JSON.stringify(data);
            if (!force && text === lastStatusText) return;
            lastStatusText = text;
            render(data);
        });
    }

    // ---- actions ----

    function onListClick(e) {
        var t = e.target;
        while (t && t !== document && !(t.getAttribute && t.getAttribute("data-act"))) t = t.parentNode;
        if (!t || t === document) return;
        var what = t.getAttribute("data-act");
        var uuid = t.getAttribute("data-uuid");
        if (what === "allow") {
            act("/allow", { uuid: uuid }, "Library allowed. Connecting…");
        } else if (what === "forget") {
            if (confirmForget !== uuid) {
                confirmForget = uuid;
                render(status);
                return;
            }
            confirmForget = null;
            act("/forget", { uuid: uuid }, "Library forgotten.");
        } else if (what === "password") {
            editingPassword = editingPassword === uuid ? null : uuid;
            renderAllowed(status.allowed);
            var input = el("pw-" + uuid);
            if (input) input.focus();
        } else if (what === "savepw") {
            var value = el("pw-" + uuid).value;
            editingPassword = null;
            act("/password", { uuid: uuid, password: value || null },
                value ? "Password saved. Reconnecting…" : "Password cleared.");
        } else if (what === "cancelpw") {
            editingPassword = null;
            render(status);
        }
    }

    function describeFind(data) {
        var servers = data.servers || [];
        if (!servers.length) {
            return "No Calibre found. Check that Calibre's wireless device connection is started and the Kindle is on the same network.";
        }
        var found = 0, notes = [];
        for (var i = 0; i < servers.length; i++) {
            var s = servers[i];
            if (s.status === "found") found++;
            else if (s.status === "error") {
                notes.push(s.hostname + ": " + (/busy/i.test(s.error)
                    ? "busy with another device. Disconnect it in Calibre and try again."
                    : s.error));
            }
        }
        var text = found ? ("Found " + found + " new librar" + (found === 1 ? "y" : "ies") + ". Tap Allow to use it.")
                         : "No new libraries found.";
        return notes.length ? text + " " + notes.join(" ") : text;
    }

    function bind() {
        el("btnConnect").onclick = function () { act("/connect-now", {}, "Connecting…"); };
        el("btnPause").onclick = function () {
            act(status && status.paused ? "/resume" : "/pause", {}, null);
        };
        el("chkAutostart").onchange = function () {
            act("/autostart", { enabled: el("chkAutostart").checked },
                el("chkAutostart").checked ? "Will start automatically." : "Won't start automatically.");
        };
        el("btnFind").onclick = function () {
            if (busy) return;
            var label = { id: "btnFind", busy: "Searching…", idle: "Find libraries" };
            setBusy(true, label);
            request("POST", "/find", {}, function (data, err) {
                setBusy(false, label);
                showMessage(err || (data.ok ? describeFind(data) : data.error));
                refresh(true);
            });
        };
        el("btnAdd").onclick = function () {
            if (busy) return;
            var host = el("inHost").value.replace(/^\s+|\s+$/g, "");
            var port = parseInt(el("inPort").value, 10);
            var label = { id: "btnAdd", busy: "Connecting…", idle: "Add server" };
            setBusy(true, label);
            request("POST", "/add-server", { host: host, port: port, password: el("inPassword").value || null },
                function (data, err) {
                    setBusy(false, label);
                    if (err) { showMessage(err); }
                    else if (!data.ok) { showMessage(data.error); }
                    else {
                        showMessage("Added " + data.library.name + ". Connecting…");
                        el("inHost").value = "";
                        el("inPassword").value = "";
                    }
                    refresh(true);
                });
        };
        el("allowedList").onclick = onListClick;
        el("seenList").onclick = onListClick;
        bindKeyboard();
    }

    // ---- on-screen keyboard ----

    // The keyboard covers the bottom of the screen, and the page doesn't move
    // out of its way. While an input has focus, pad the page so any field can
    // scroll to the top, then scroll it there.
    function isInput(t) { return t && t.tagName === "INPUT" && t.type !== "checkbox"; }

    function bindKeyboard() {
        document.addEventListener("focus", function (e) {
            if (!isInput(e.target)) return;
            document.body.style.paddingBottom = window.innerHeight + "px";
            var t = e.target;
            setTimeout(function () {
                var y = 0;
                for (var n = t; n; n = n.offsetParent) y += n.offsetTop;
                window.scrollTo(0, Math.max(0, y - window.innerHeight / 10));
            }, 50);
        }, true);
        document.addEventListener("blur", function () {
            // Moving to the next field blurs then focuses: keep the padding.
            setTimeout(function () {
                if (!isInput(document.activeElement)) document.body.style.paddingBottom = "";
            }, 50);
        }, true);
        // Enter moves to the next field in the same card; on the last one it
        // closes the keyboard so the buttons are visible again.
        document.addEventListener("keydown", function (e) {
            if (e.keyCode !== 13 || !isInput(e.target)) return;
            e.preventDefault();
            var card = e.target;
            while (card && !/\bcard\b/.test(card.className)) card = card.parentNode;
            var inputs = card ? card.getElementsByTagName("input") : [];
            var next = null;
            for (var i = 0; i < inputs.length - 1; i++) {
                if (inputs[i] === e.target && isInput(inputs[i + 1])) next = inputs[i + 1];
            }
            if (next) next.focus();
            else e.target.blur();
        }, true);
    }

    function init() {
        bind();
        refresh(true);
        setInterval(function () { if (!busy) refresh(false); }, POLL_MS);
    }

    if (document.readyState === "complete" || document.readyState === "interactive") init();
    else document.addEventListener("DOMContentLoaded", init, false);
})();
