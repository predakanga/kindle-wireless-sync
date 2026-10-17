/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Prototype of the Wireless Sync app as one screen per page (see
 * ../README.md). It talks to a fake service: the harness's, through
 * postMessage, or its own (mock.js) when opened alone. ES5 only: the
 * Kindle's mesquite engine has no fetch, promises or arrow functions.
 */
(function () {
    "use strict";

    var POLL_MS = 1500;
    var MESSAGE_MS = 5000;

    var status = null;          // last /status, or {error} when the service is down
    var lastStatusText = null;
    var page = null;            // home, choose, address, probe, setup, picker, settings, password
    var renderedKey = null;     // form pages aren't redrawn under the user's typing
    var form = null;            // settings being edited: {ctx: "setup"|"settings", ...}
    var pickerKind = null;      // "bool" or "datetime"
    var dialog = null;          // {kind: "unlink"|"replace", ...}
    var lastProbe = null;       // the /probe body, for Try again
    var probeStarting = false;  // sent /probe; its status hasn't arrived yet
    var probeFrom = "choose";   // where Cancel goes from a probe
    var passwordFrom = "settings";
    var addr = { host: "", port: "9090", password: "" };
    var messageTimer = null;

    // ---- transport ----

    function xhrRequest(method, path, body, cb) {
        var xhr = new XMLHttpRequest();
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4) return;
            if (xhr.status === 0) return cb(null, "The service isn't running.");
            var data = null;
            try { data = JSON.parse(xhr.responseText); } catch (e) { /* below */ }
            if (!data) return cb(null, "Unexpected reply (HTTP " + xhr.status + ").");
            cb(data, null);
        };
        xhr.open(method, "http://127.0.0.1:8325" + path, true);
        xhr.setRequestHeader("Content-Type", "text/plain");
        xhr.send(body === null ? null : JSON.stringify(body));
    }

    var request = (function () {
        var framed = false;
        try { framed = window.parent && window.parent !== window; } catch (e) { framed = true; }
        if (framed) {
            var pending = {}, next = 1;
            window.addEventListener("message", function (e) {
                var d = e.data || {};
                if (d.kws === "poke") { refresh(); return; }
                if (d.kws !== "res") return;
                var cb = pending[d.id];
                delete pending[d.id];
                if (cb) cb(d.data, d.err);
            }, false);
            return function (method, path, body, cb) {
                var id = next++;
                pending[id] = cb;
                window.parent.postMessage({ kws: "req", id: id, method: method, path: path, body: body }, "*");
            };
        }
        if (window.KWSMock) {
            var mock = new window.KWSMock(function () {});
            return function (method, path, body, cb) { mock.request(method, path, body, cb); };
        }
        return xhrRequest;
    })();

    function post(path, body, cb) {
        request("POST", path, body || {}, function (data, err) {
            if (err) showMessage(err);
            else if (!data.ok && !data.needs_replace) showMessage(data.error || "That didn't work.");
            if (cb) cb(data, err);
            refresh();
        });
    }

    // ---- helpers ----

    function el(id) { return document.getElementById(id); }

    function esc(s) {
        if (s === null || s === undefined) return "";
        return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;")
            .replace(/>/g, "&gt;").replace(/"/g, "&quot;");
    }

    function showMessage(text) {
        var m = el("message");
        m.innerHTML = esc(text);
        m.className = "message visible";
        if (messageTimer) clearTimeout(messageTimer);
        messageTimer = setTimeout(function () { m.className = "message"; }, MESSAGE_MS);
    }

    function variant(key) {
        return status && status.prototype && status.prototype.variants ? status.prototype.variants[key] : null;
    }

    function address(lib) { return lib.host + ":" + lib.port; }
    function where(lib) { return (lib.hostname ? lib.hostname + " · " : "") + address(lib); }

    function columnName(columns, lookup) {
        if (!lookup) return "Don't sync";
        var all = (columns && columns.bool || []).concat(columns && columns.datetime || []);
        for (var i = 0; i < all.length; i++) if (all[i].lookup === lookup) return all[i].name;
        return lookup + " (missing)";
    }

    function hasColumn(columns, kind, lookup) {
        var list = columns && columns[kind] || [];
        for (var i = 0; i < list.length; i++) if (list[i].lookup === lookup) return true;
        return false;
    }

    // ---- building blocks ----

    function header(title, backAct) {
        return '<div class="header">'
            + (backAct ? '<button class="back" data-act="' + backAct + '">‹</button>' : '')
            + '<div class="title' + (backAct ? '' : ' no-back') + '">' + esc(title) + '</div></div>';
    }

    function toggleRow(label, on, act, boxed) {
        return '<div class="toggle-row' + (boxed ? ' boxed' : '') + '">'
            + '<span class="toggle-label">' + esc(label) + '</span>'
            + '<button class="toggle' + (on ? ' on' : '') + '" data-act="' + act + '"><span class="knob"></span></button></div>';
    }

    function button(label, act, opts) {
        opts = opts || {};
        return '<button class="btn' + (opts.cls ? ' ' + opts.cls : '') + '" data-act="' + act + '"'
            + (opts.arg !== undefined ? ' data-arg="' + esc(opts.arg) + '"' : '')
            + (opts.disabled ? ' disabled="disabled"' : '') + '>' + esc(label) + '</button>';
    }

    function btnRow(buttons) {
        return '<div class="btn-row' + (buttons.length === 3 ? ' thirds' : '') + '">' + buttons.join("") + '</div>';
    }

    function sectionLabel(text, aside) {
        return '<div class="section-label">' + esc(text)
            + (aside ? '<span class="aside">' + esc(aside) + '</span>' : '') + '</div>';
    }

    function field(id, label, type, value) {
        return '<div class="field"><label for="' + id + '">' + esc(label) + '</label>'
            + '<input type="' + type + '" id="' + id + '" value="' + esc(value || "") + '" autocomplete="off" autocapitalize="off" /></div>';
    }

    function footer() {
        var html = '';
        if (variant("serviceControls") === "bottom") {
            html += '<div class="rule service"><span class="label">Service running</span>'
                + button("Stop", "service", { cls: "btn-small", arg: "stop" })
                + button("Restart", "service", { cls: "btn-small", arg: "restart" }) + '</div>';
        }
        return html + '<div class="footer">Wireless Sync ' + esc(status.version || "") + '</div>';
    }

    // ---- pages ----

    var STATE_TITLES = {
        waiting_for_wifi: "Waiting for Wi-Fi",
        not_linked: "No library linked",
        searching: "Connecting",
        not_found: "Not connected",
        connected: "Connected",
        ejected: "Disconnected by Calibre",
        password_rejected: "Password rejected",
        error: "Problem",
        paused: "Paused"
    };

    function readSyncText(lib) {
        if (!lib.writable) return "Read status isn't sent to Calibre";
        var parts = [];
        if (lib.read_column) parts.push(columnName(lib.columns, lib.read_column));
        if (lib.last_read_column) parts.push(columnName(lib.columns, lib.last_read_column));
        return parts.length ? "Read status goes to " + parts.join(" and ") : "Read sync on, but no columns chosen";
    }

    var PAGES = {};
    var BOTTOM = {};

    PAGES.home = function (s) {
        var lib = s.library;
        var canConnect = !s.paused && s.state !== "connected" && s.state !== "searching" && s.state !== "waiting_for_wifi";
        var html = toggleRow("Start automatically", s.autostart, "autostart")
            + sectionLabel("Status")
            + '<div class="state">' + esc(STATE_TITLES[s.state] || s.state) + '</div>'
            + '<div class="detail">' + esc(s.detail) + '</div>';
        if (s.state === "password_rejected") {
            html += '<div class="hint">Calibre\'s password may have changed.</div>'
                + button("Enter the password", "password", { cls: "btn-wide btn-strong", arg: "home" });
        } else if (s.state === "not_found") {
            html += '<div class="hint">Is Calibre open, with Connect/share → Start wireless device connection on?</div>';
        }
        html += btnRow([button("Connect now", "connect", { disabled: !canConnect }),
                        button(s.paused ? "Resume" : "Pause", "pause")])
            + sectionLabel("Calibre library")
            + '<div class="rows"><div class="row" data-act="go" data-arg="settings">'
            + '<div class="name">' + esc(lib.name) + '</div>'
            + '<div class="sub">' + esc(where(lib)) + '</div>'
            + '<div class="sub">' + esc(readSyncText(lib)) + '</div>'
            + '<span class="chev">›</span></div></div>';
        if (variant("changeFlow") === "choose") {
            html += btnRow([button("Library settings", "go", { arg: "settings" }),
                            button("Change library", "change")]);
        } else {
            html += '<div class="btn-row">' + button("Library settings", "go", { cls: "btn-wide", arg: "settings" }) + '</div>';
        }
        return html;
    };
    BOTTOM.home = footer;

    PAGES.choose = function (s) {
        var root = !s.library;
        var html = root
            ? toggleRow("Start automatically", s.autostart, "autostart") + '<h1>Link a Calibre library</h1>'
            : header("Choose a library", "back:home");
        html += '<div class="hint">In Calibre, choose <b>Connect/share → Start wireless device connection</b>. '
            + 'Then tap the library to use.</div>'
            + sectionLabel("Found on the network", s.finding ? "Searching…" : "");
        var found = s.found || [];
        if (!found.length) {
            html += '<div class="empty">' + (s.finding ? "Looking for Calibre…" : "No libraries found.") + '</div>';
        } else {
            html += '<div class="rows">';
            for (var i = 0; i < found.length; i++) {
                var f = found[i];
                html += '<div class="row" data-act="probe" data-arg="' + esc(f.uuid) + '">'
                    + '<div class="name">' + esc(f.name) + '</div><div class="sub">' + esc(where(f)) + '</div>'
                    + '<span class="chev">›</span></div>';
            }
            html += '</div>';
        }
        html += btnRow([button("Find again", "find", { disabled: s.finding }),
                        button("Link by address…", "go", { arg: "address" })]);
        if (!root && variant("changeFlow") === "choose") {
            html += '<div class="hint">Linking another library replaces ' + esc(s.library.name) + '.</div>';
        }
        return html;
    };
    BOTTOM.choose = function (s) { return s.library ? "" : footer(); };

    PAGES.address = function () {
        return header("Link by address", "back:choose")
            + '<div class="hint">For when Find can\'t see your computer. In Calibre\'s wireless settings, '
            + 'tick <b>Use fixed network port</b> so the port doesn\'t change.</div>'
            + field("inHost", "Computer name or address", "text", addr.host)
            + field("inPort", "Port", "number", addr.port)
            + field("inPassword", "Password (if Calibre has one)", "password", addr.password)
            + btnRow([button("Check", "checkaddress", { cls: "btn-strong" }), button("Cancel", "back:choose")]);
    };

    PAGES.probe = function (s) {
        var p = s.probe || { state: "checking", target: {} };
        var name = p.target.name || address(p.target);
        if (p.state === "checking") {
            return header("Checking", null)
                + '<h1>Checking ' + esc(name) + '…</h1>'
                + '<div class="detail">Wireless Sync is connecting to Calibre to check the password and '
                + 'read the library\'s columns. Nothing is synced yet.</div>'
                + '<div class="hint">Calibre shows the Kindle as connected for a few seconds.</div>'
                + button("Cancel", "cancelprobe", { cls: "btn-wide" });
        }
        if (p.state === "password_needed" || p.state === "password_rejected") {
            return header(name, "cancelprobe")
                + '<h1>' + (p.state === "password_needed" ? "Password needed" : "Wrong password") + '</h1>'
                + '<div class="detail">' + (p.state === "password_needed"
                    ? esc(name) + ' asks for a password.'
                    : 'Calibre rejected that password.') + '</div>'
                + '<div class="hint">It\'s set in Calibre\'s <b>Start wireless device connection</b> window.</div>'
                + field("inProbePw", "Password", "password", "")
                + btnRow([button("Check again", "probepw", { cls: "btn-strong" }), button("Cancel", "cancelprobe")]);
        }
        return header(name, "cancelprobe")
            + '<h1>Couldn\'t check ' + esc(name) + '</h1>'
            + '<div class="detail">' + esc(p.error || "Something went wrong.") + '</div>'
            + btnRow([button("Try again", "retryprobe", { cls: "btn-strong" }), button("Back", "cancelprobe")]);
    };

    function pickerRow(label, kind, columns) {
        var lookup = kind === "bool" ? form.read_column : form.last_read_column;
        var none = !(columns && columns[kind] && columns[kind].length);
        return '<div class="row" data-act="picker" data-arg="' + kind + '">'
            + '<span class="name">' + esc(label) + '</span>'
            + '<span class="value">' + esc(none && !lookup ? "No columns yet" : columnName(columns, lookup)) + '</span>'
            + '<span class="chev">›</span></div>';
    }

    function readSyncForm(columns) {
        var html = toggleRow("Send read status to Calibre", form.writable, "writable", true);
        if (!form.writable) {
            return html + '<div class="hint">When on, books marked read on the Kindle, and when you last read them, '
                + 'are saved in Calibre columns you choose. Off, the Kindle never changes this library.</div>';
        }
        return html + '<div class="rows" style="border-top:none">'
            + pickerRow("Read", "bool", columns) + pickerRow("Last read", "datetime", columns) + '</div>';
    }

    PAGES.setup = function (s) {
        var lib = s.probe.library;
        var replacing = s.library && s.library.uuid !== lib.uuid;
        var inline = replacing && variant("replaceConfirm") === "inline";
        var html = header("Link " + lib.name, "cancelprobe")
            + '<div class="sub">' + esc(where(lib)) + '</div>';
        if (inline) {
            html += '<div class="warning">This replaces <b>' + esc(s.library.name) + '</b>. The books stay on the Kindle, '
                + 'but their details from ' + esc(s.library.name) + ' (series, read status) are forgotten until '
                + esc(lib.name) + ' sends its own.</div>';
        }
        html += readSyncForm(lib.columns)
            + btnRow([button(inline ? "Replace and link" : "Link", "link", { cls: "btn-strong" }),
                      button("Cancel", "cancelprobe")]);
        return html;
    };

    PAGES.picker = function (s) {
        var columns = form.ctx === "setup" ? s.probe.library.columns : s.library.columns;
        var libName = form.ctx === "setup" ? s.probe.library.name : s.library.name;
        var isRead = pickerKind === "bool";
        var current = isRead ? form.read_column : form.last_read_column;
        var list = columns && columns[pickerKind] || [];
        var html = header(isRead ? "Read column" : "Last-read column", "back:" + form.ctx)
            + '<div class="hint">' + (isRead ? "Yes/No" : "Date") + ' columns in ' + esc(libName) + '.</div>';
        html += '<div class="rows"><div class="row plain" data-act="pick" data-arg="">'
            + '<span class="mark' + (!current ? ' on' : '') + '"></span>Don\'t sync</div>';
        for (var i = 0; i < list.length; i++) {
            html += '<div class="row plain" data-act="pick" data-arg="' + esc(list[i].lookup) + '">'
                + '<span class="mark' + (current === list[i].lookup ? ' on' : '') + '"></span>'
                + esc(list[i].name) + ' <span class="sub">' + esc(list[i].lookup) + '</span></div>';
        }
        html += '</div>';
        if (!list.length) {
            html += '<div class="hint">' + esc(libName) + ' has no ' + (isRead ? "Yes/No" : "Date") + ' columns. '
                + 'In Calibre, add one under <b>Preferences → Add your own columns</b>'
                + (isRead ? ' (type "Yes/No")' : ' (type "Date")') + ', for example "'
                + (isRead ? "Kindle read" : "Kindle last read") + '".</div>';
            html += form.ctx === "setup"
                ? button("Check again", "retryprobe", { cls: "btn-wide" })
                : '<div class="hint">It shows up here after Wireless Sync next connects.</div>';
        }
        return html;
    };

    PAGES.settings = function (s) {
        var lib = s.library;
        var changed = form.writable !== lib.writable || form.read_column !== lib.read_column
            || form.last_read_column !== lib.last_read_column;
        return header(lib.name, "back:home")
            + '<div class="fact"><span class="k">Computer</span><span class="v">' + esc(lib.hostname) + '</span></div>'
            + '<div class="fact"><span class="k">Address</span><span class="v">' + esc(address(lib)) + '</span></div>'
            + '<div class="fact"><span class="k">Password</span><span class="v">' + (lib.has_password ? "Set" : "None")
            + button("Change", "password", { cls: "btn-small", arg: "settings" }) + '</span></div>'
            + readSyncForm(lib.columns)
            + (form.writable ? '<div class="hint">Columns as of the last connection to Calibre.</div>' : '')
            + button("Save", "save", { cls: "btn-wide btn-strong", disabled: !changed });
    };
    BOTTOM.settings = function (s) {
        return '<div class="rule">'
            + (variant("changeFlow") === "unlink" ? '<div class="hint">To use a different library, unlink this one first.</div>' : '')
            + button("Unlink " + s.library.name, "unlink", { cls: "btn-wide" }) + '</div>';
    };

    PAGES.password = function (s) {
        return header("Password", "back:" + passwordFrom)
            + '<div class="detail">The password for ' + esc(s.library.name) + ', as set in Calibre\'s '
            + '<b>Start wireless device connection</b> window.</div>'
            + field("inPw", "Password", "password", "")
            + btnRow([button("Save", "savepw", { cls: "btn-strong" }), button("No password", "clearpw")]);
    };

    PAGES.stopped = function (s) {
        return '<h1>Service not running</h1><div class="detail">' + esc(s.error)
            + ' Close and reopen Wireless Sync to start it.</div>';
    };

    // ---- dialogs ----

    function renderDialog() {
        var d = el("dialog");
        if (!dialog) { d.className = "overlay hidden"; d.innerHTML = ""; return; }
        var html = '<div class="dialog">';
        if (dialog.kind === "unlink") {
            html += '<div class="title">Unlink ' + esc(dialog.name) + '?</div>'
                + '<div class="detail">The books stay on the Kindle. Wireless Sync stops connecting to '
                + esc(dialog.name) + ' until you link a library again.</div>'
                + btnRow([button("Cancel", "closedialog"), button("Unlink", "dounlink", { cls: "btn-strong" })]);
        } else if (dialog.kind === "replace") {
            html += '<div class="title">Replace ' + esc(dialog.linked) + '?</div>'
                + '<div class="detail">This Kindle is linked to <b>' + esc(dialog.linked) + '</b>. Linking <b>'
                + esc(dialog.name) + '</b> replaces it. The books stay on the Kindle, but their details from '
                + esc(dialog.linked) + ' (series, read status) are forgotten until ' + esc(dialog.name)
                + ' sends its own.</div>'
                + btnRow([button("Cancel", "closedialog"), button("Replace", "doreplace", { cls: "btn-strong" })]);
        }
        d.innerHTML = html + '</div>';
        d.className = "overlay";
    }

    // ---- navigation and rendering ----

    function settingsForm(lib) {
        return { ctx: "settings", writable: lib.writable, read_column: lib.read_column,
                 last_read_column: lib.last_read_column };
    }

    function setupForm(lib, old) {
        if (old && old.ctx === "setup" && old.uuid === lib.uuid) return old;
        var cols = lib.columns;
        return { ctx: "setup", uuid: lib.uuid, writable: false,
                 read_column: hasColumn(cols, "bool", "#kindle_read") ? "#kindle_read" : null,
                 last_read_column: hasColumn(cols, "datetime", "#kindle_last_read") ? "#kindle_last_read" : null };
    }

    function go(to) {
        // Back from a column picker keeps the edits; otherwise start from the saved settings.
        if (to === "settings" && status.library && page !== "picker") form = settingsForm(status.library);
        if (to === "choose") startFind();
        page = to;
        redraw();
    }

    function fixPage(s) {
        if (s.error) { page = "stopped"; return; }
        if (page === "stopped" || !page) {
            page = s.library ? "home" : "choose";
            if (!s.library) startFind();
        }
        if (!s.library && (page === "home" || page === "settings" || page === "password")) {
            page = "choose";
            startFind();
        }
        var probing = page === "probe" || page === "setup" || (page === "picker" && form && form.ctx === "setup");
        if (s.probe) probeStarting = false;
        if (probing && !s.probe && !probeStarting) page = probeFrom;
        if (page === "probe" && s.probe && s.probe.state === "ok") {
            form = setupForm(s.probe.library, form);
            page = "setup";
        }
        if (page === "picker" && !form) page = s.library ? "home" : "choose";
    }

    var FORM_PAGES = { address: true, password: true };

    function render() {
        if (!status) return;
        fixPage(status);
        var probeState = page === "probe" && status.probe ? status.probe.state : "";
        var key = page + "|" + probeState;
        var typing = FORM_PAGES[page] || probeState === "password_needed" || probeState === "password_rejected";
        if (!(typing && key === renderedKey)) {
            el("page").innerHTML = PAGES[page](status);
            el("bottom").innerHTML = BOTTOM[page] && !status.error ? BOTTOM[page](status) : "";
            renderedKey = key;
        }
        renderDialog();
        report();
    }

    function redraw() { renderedKey = null; render(); }

    /// Tell the harness how much the page overflows the screen.
    function report() {
        if (window.parent === window) return;
        var pageEl = el("page"), bottom = el("bottom");
        var used = pageEl.offsetTop + pageEl.offsetHeight;
        var room = window.innerHeight - (bottom.innerHTML ? bottom.offsetHeight : 0);
        window.parent.postMessage({ kws: "metrics", page: page + (dialog ? " + " + dialog.kind + " dialog" : ""),
                                    overflow: used - room }, "*");
    }

    function refresh() {
        request("GET", "/status", null, function (data, err) {
            var next = err ? { error: err } : data;
            var text = JSON.stringify(next);
            if (text === lastStatusText) return; // e-ink: only redraw on change
            lastStatusText = text;
            status = next;
            render();
        });
    }

    function startFind() { post("/find", {}); }

    // ---- actions ----

    function startProbe(body, from) {
        lastProbe = body;
        probeFrom = from;
        probeStarting = true;
        page = "probe";
        post("/probe", body);
        redraw();
    }

    function doLink(replace) {
        post("/link", { writable: form.writable, read_column: form.writable ? form.read_column : null,
                        last_read_column: form.writable ? form.last_read_column : null, replace: !!replace },
            function (data) {
                if (data && data.needs_replace) {
                    dialog = { kind: "replace", linked: data.linked.name, name: data.library.name };
                    renderDialog();
                    report();
                } else if (data && data.ok) {
                    showMessage("Linked to " + status.probe.library.name + ".");
                    form = null;
                    page = "home";
                }
            });
    }

    var ACTIONS = {
        go: function (arg) { go(arg); },
        autostart: function () { post("/autostart", { enabled: !status.autostart }); },
        connect: function () { post("/connect-now"); },
        pause: function () { post(status.paused ? "/resume" : "/pause"); },
        find: function () { startFind(); },
        change: function () { go("choose"); },
        probe: function (uuid) { startProbe({ uuid: uuid }, "choose"); },
        checkaddress: function () {
            addr = { host: el("inHost").value.replace(/^\s+|\s+$/g, ""), port: el("inPort").value,
                     password: el("inPassword").value };
            if (!addr.host) { showMessage("Enter the computer's name or address."); return; }
            startProbe({ host: addr.host, port: parseInt(addr.port, 10) || 9090, password: addr.password || null }, "address");
        },
        probepw: function () {
            var body = {};
            for (var k in lastProbe) if (lastProbe.hasOwnProperty(k)) body[k] = lastProbe[k];
            body.password = el("inProbePw").value;
            startProbe(body, probeFrom);
        },
        retryprobe: function () { startProbe(lastProbe, probeFrom); },
        cancelprobe: function () {
            post("/probe/cancel");
            form = null;
            page = probeFrom;
            if (page === "choose") startFind();
            redraw();
        },
        writable: function () { form.writable = !form.writable; redraw(); },
        picker: function (kind) { pickerKind = kind; page = "picker"; redraw(); },
        pick: function (lookup) {
            if (pickerKind === "bool") form.read_column = lookup || null;
            else form.last_read_column = lookup || null;
            page = form.ctx;
            redraw();
        },
        link: function () { doLink(variant("replaceConfirm") === "inline"); },
        doreplace: function () { dialog = null; renderDialog(); doLink(true); },
        closedialog: function () { dialog = null; renderDialog(); report(); },
        save: function () {
            post("/settings", { writable: form.writable, read_column: form.read_column,
                                last_read_column: form.last_read_column }, function (data) {
                if (data && data.ok) { showMessage("Saved. Reconnecting to Calibre to use them."); page = "home"; }
            });
        },
        password: function (from) { passwordFrom = from; page = "password"; redraw(); },
        savepw: function () {
            post("/password", { password: el("inPw").value || null }, function (data) {
                if (data && data.ok) { showMessage("Password saved."); page = passwordFrom; redraw(); }
            });
        },
        clearpw: function () {
            post("/password", { password: null }, function (data) {
                if (data && data.ok) { showMessage("Password cleared."); page = passwordFrom; redraw(); }
            });
        },
        unlink: function () { dialog = { kind: "unlink", name: status.library.name }; renderDialog(); report(); },
        dounlink: function () {
            dialog = null;
            post("/unlink", {}, function (data) {
                if (data && data.ok) showMessage("Unlinked. The books stay on the Kindle.");
            });
        },
        service: function (what) { post("/service", { action: what }); }
    };

    function onClick(e) {
        var t = e.target;
        while (t && t !== document && !(t.getAttribute && t.getAttribute("data-act"))) t = t.parentNode;
        if (!t || t === document || t.disabled) return;
        var act = t.getAttribute("data-act");
        var arg = t.getAttribute("data-arg");
        if (act.indexOf("back:") === 0) { go(act.substring(5)); return; }
        if (ACTIONS[act]) ACTIONS[act](arg);
    }

    // The Kindle's keyboard covers the bottom of the screen: show roughly how much.
    function onFocus(e) {
        var t = e.target;
        var typing = t && t.tagName === "INPUT";
        var show = typing && status && status.prototype && status.prototype.keyboard;
        el("keyboard").className = show ? "keyboard" : "keyboard hidden";
    }

    document.addEventListener("click", onClick, false);
    document.addEventListener("focus", onFocus, true);
    document.addEventListener("blur", function () { setTimeout(function () { onFocus({ target: document.activeElement }); }, 0); }, true);
    refresh();
    setInterval(refresh, POLL_MS);
})();
