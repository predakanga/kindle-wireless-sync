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
    var busy = false;          // a slow action (find, link) is running
    var confirmUnlink = false; // awaiting a second tap on Unlink
    var editingPassword = null; // uuid whose password form is open
    var pendingReplace = null; // a link that would replace the linked library (ADR 0018)
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
        not_linked: "No library linked",
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
            && s.state !== "waiting_for_wifi" && s.state !== "not_linked";
        el("btnConnect").disabled = !canConnect;
        el("btnPause").innerHTML = s.paused ? "Resume" : "Pause";
        // Nothing to pause until a library is linked; Resume always works.
        el("btnPause").disabled = !s.paused && s.state === "not_linked";
        el("chkAutostart").checked = !!s.autostart;
        show("guardWarning", !!s.autostart_disabled_by_guard && !s.autostart);
        if (s.config_error) showMessage("Settings file problem: " + s.config_error);
    }

    function renderLinked(a) {
        if (!a) {
            el("linked").innerHTML = '<div class="empty">No library linked yet. Use Find libraries below, or link one by address.</div>';
            return;
        }
        var u = esc(a.uuid);
        var html = '<div class="item">'
            + '<div class="item-head"><span class="item-name">' + esc(a.name || a.uuid) + '</span>'
            + '<button class="btn btn-small" data-act="password" data-uuid="' + u + '">Password</button>'
            + '<button class="btn btn-small' + (confirmUnlink ? ' btn-danger' : '') + '" data-act="unlink" data-uuid="' + u + '">'
            + (confirmUnlink ? 'Tap to confirm' : 'Unlink') + '</button></div>'
            + '<div class="item-sub">' + esc(address(a)) + (a.has_password ? " · password set" : "")
            + (a.writable ? " · sends read status" : "") + '</div>';
        if (editingPassword === a.uuid) {
            html += '<div class="item-form">'
                + '<input type="password" id="pw-' + u + '" placeholder="Calibre password (empty to clear)" />'
                + '<div class="row" style="margin-top:0.5rem">'
                + '<button class="btn" data-act="savepw" data-uuid="' + u + '">Save</button>'
                + '<button class="btn" data-act="cancelpw">Cancel</button></div></div>';
        }
        el("linked").innerHTML = html + '</div>';
    }

    function renderFound(list) {
        show("foundHeading", !!(list && list.length));
        var html = "";
        for (var i = 0; list && i < list.length; i++) {
            var f = list[i];
            html += '<div class="item"><div class="item-head"><span class="item-name">' + esc(f.name || f.uuid) + '</span>'
                + '<button class="btn btn-small" data-act="link" data-uuid="' + esc(f.uuid) + '">Link</button></div>'
                + '<div class="item-sub">' + esc((f.hostname ? f.hostname + " · " : "") + address(f)) + '</div></div>';
        }
        el("foundList").innerHTML = html;
    }

    function renderReplace() {
        show("replaceBox", !!pendingReplace);
        if (!pendingReplace) return;
        var r = pendingReplace;
        el("replaceBox").innerHTML = 'This Kindle is linked to <b>' + esc(r.linked) + '</b>. Linking <b>'
            + esc(r.name) + '</b> replaces it. The books stay on the Kindle, but their details from '
            + esc(r.linked) + ' (tags, read status) are forgotten until ' + esc(r.name) + ' sends its own.'
            + '<div class="row" style="margin-top:0.5rem">'
            + '<button class="btn btn-danger" data-act="confirmreplace">Link ' + esc(r.name) + '</button>'
            + '<button class="btn" data-act="cancelreplace">Cancel</button></div>';
    }

    function render(s) {
        status = s;
        renderStatus(s);
        renderReplace();
        // Don't redraw lists under an open password form: it would eat input.
        if (!editingPassword) {
            renderLinked(s.library);
            renderFound(s.found);
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

    /// POST /link. If it would replace the linked library, ask first; the
    /// confirmation resends it with replace, pinned to the same library.
    function link(body, done) {
        request("POST", "/link", body, function (data, err) {
            if (err) { showMessage(err); }
            else if (data.needs_replace) {
                var again = {};
                for (var k in body) if (body.hasOwnProperty(k)) again[k] = body[k];
                again.uuid = data.library.uuid;
                again.replace = true;
                pendingReplace = {
                    body: again,
                    linked: data.linked.name || data.linked.uuid,
                    name: data.library.name || data.library.uuid
                };
                renderReplace();
                // Linking by address asks from the card below: bring the question into view.
                try { el("replaceBox").scrollIntoView(); } catch (e) { /* not essential */ }
            }
            else if (!data.ok) { showMessage(data.error || "That didn't work."); }
            else { showMessage("Linked to " + (data.library.name || data.library.uuid) + ". Connecting…"); }
            if (done) done(data, err);
            refresh(true);
        });
    }

    function onListClick(e) {
        var t = e.target;
        while (t && t !== document && !(t.getAttribute && t.getAttribute("data-act"))) t = t.parentNode;
        if (!t || t === document) return;
        var what = t.getAttribute("data-act");
        var uuid = t.getAttribute("data-uuid");
        if (what === "link") {
            pendingReplace = null;
            link({ uuid: uuid });
        } else if (what === "confirmreplace") {
            var body = pendingReplace.body;
            pendingReplace = null;
            renderReplace();
            if (!body.host) { link(body); return; }
            // By address: this probes Calibre again, which can take a while.
            if (busy) return;
            var label = { id: "btnAdd", busy: "Connecting…", idle: "Link" };
            setBusy(true, label);
            link(body, function () { setBusy(false, label); });
        } else if (what === "cancelreplace") {
            pendingReplace = null;
            renderReplace();
        } else if (what === "unlink") {
            if (!confirmUnlink) {
                confirmUnlink = true;
                render(status);
                return;
            }
            confirmUnlink = false;
            act("/unlink", { uuid: uuid }, "Library unlinked. The books stay on the Kindle.");
        } else if (what === "password") {
            editingPassword = editingPassword === uuid ? null : uuid;
            renderLinked(status.library);
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
        var text = found ? ("Found " + found + " other librar" + (found === 1 ? "y" : "ies") + ". Tap Link to use it.")
                         : "No other libraries found.";
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
            var label = { id: "btnAdd", busy: "Connecting…", idle: "Link" };
            pendingReplace = null;
            setBusy(true, label);
            link({ host: host, port: port, password: el("inPassword").value || null },
                function (data) {
                    setBusy(false, label);
                    if (data && (data.ok || data.needs_replace)) {
                        el("inHost").value = "";
                        el("inPassword").value = "";
                    }
                });
        };
        el("linked").onclick = onListClick;
        el("foundList").onclick = onListClick;
        el("replaceBox").onclick = onListClick;
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
