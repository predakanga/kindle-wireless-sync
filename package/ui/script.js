/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Wireless Sync settings app: one screen per page (ADR 0020), talking to the
 * service's localhost API (ADR 0011). ES5 only: the Kindle's mesquite engine
 * has no fetch, promises or arrow functions. The design was settled with
 * tools/ui-prototype.
 */
(function () {
    "use strict";

    var API = "http://127.0.0.1:8325";
    var POLL_MS = 3000;
    var MESSAGE_MS = 6000;

    var status = null;          // last /status, or {error} when the service is down
    var lastStatusText = null;
    var page = null;            // see PAGES
    var renderedKey = null;     // form pages aren't redrawn under the user's typing
    var form = null;            // settings being edited: {ctx: "setup"|"settings", ...}
    var pickerKind = null;      // "bool" or "datetime"
    var dialog = null;          // {kind: "unlink"|"replace", ...}
    var lastProbe = null;       // the /probe body, for Try again
    var probeStarting = false;  // sent /probe; its status hasn't arrived yet
    var probeFrom = "choose";   // where Back goes from a check
    var passwordFrom = "settings";
    var appFrom = "home";       // where Back goes from Settings
    var addressFrom = "choose"; // where Back goes from Connect by address
    var addr = { host: "", port: "9090", password: "" };
    var finding = false;        // /find is running
    var findNote = "";          // what the last search couldn't reach
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

    /// POST, reporting failures ({ok: false}) as messages.
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

    function address(lib) { return lib.host + (lib.port ? ":" + lib.port : ""); }
    function where(lib) { return (lib.hostname ? lib.hostname + " · " : "") + address(lib); }

    /// A library's columns of one type ("bool", "datetime"), by heading.
    function columnList(columns, kind) {
        var out = [];
        for (var lookup in columns) {
            if (columns.hasOwnProperty(lookup) && columns[lookup].datatype === kind) {
                out.push({ lookup: lookup, name: columns[lookup].name || lookup });
            }
        }
        out.sort(function (a, b) { return a.name < b.name ? -1 : a.name > b.name ? 1 : 0; });
        return out;
    }

    function columnName(columns, lookup) {
        if (!lookup) return "Don't sync";
        var c = columns && columns[lookup];
        return c ? (c.name || lookup) : lookup + " (missing)";
    }

    function hasColumn(columns, kind, lookup) {
        return !!(columns && columns[lookup] && columns[lookup].datatype === kind);
    }

    /// The first page: Home once linked, otherwise the list of libraries.
    function rootPage() { return status && status.library ? "home" : "choose"; }

    // ---- building blocks ----

    // The Kindle's fonts have no ⚙, so the gear is drawn: eight teeth round a
    // ring. currentColor keeps it visible when the button is pressed.
    var GEAR = '<svg class="gear-icon" viewBox="0 0 24 24" fill="currentColor">'
        + (function () {
            var teeth = "";
            for (var a = 0; a < 360; a += 45) {
                teeth += '<rect x="10.2" y="1.2" width="3.6" height="5" transform="rotate(' + a + ' 12 12)"/>';
            }
            return teeth;
        })()
        + '<path fill-rule="evenodd" d="M12 4.5a7.5 7.5 0 1 0 0 15a7.5 7.5 0 1 0 0-15zM12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6z"/>'
        + '</svg>';

    /// A page header: a back button (backAct) or none, and a gear for
    /// Settings on the first pages.
    function header(title, backAct, gear) {
        return '<div class="header">'
            + (backAct ? '<button class="back" data-act="' + backAct + '">‹</button>' : '')
            + '<div class="title' + (backAct ? '' : ' no-back') + (gear ? ' with-gear' : '') + '">' + esc(title) + '</div>'
            + (gear ? '<button class="gear" data-act="appsettings">' + GEAR + '</button>' : '')
            + '</div>';
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
        return '<div class="btn-row">' + buttons.join("") + '</div>';
    }

    /// A button alone on its row: full width.
    function wide(label, act, opts) {
        opts = opts || {};
        opts.cls = "btn-wide" + (opts.cls ? " " + opts.cls : "");
        return '<div class="btn-row">' + button(label, act, opts) + '</div>';
    }

    function sectionLabel(text, aside) {
        return '<div class="section-label">' + esc(text)
            + (aside ? '<span class="aside">' + esc(aside) + '</span>' : '') + '</div>';
    }

    function field(id, label, type, value) {
        return '<div class="field"><label for="' + id + '">' + esc(label) + '</label>'
            + '<input type="' + type + '" id="' + id + '" value="' + esc(value || "") + '" autocomplete="off" autocapitalize="off" /></div>';
    }

    function fact(k, v) {
        return '<div class="fact"><span class="k">' + esc(k) + '</span><span class="v">' + v + '</span></div>';
    }

    function forgetText(oldName, newName) {
        return 'Your books and their series stay on the Kindle. The rest of what ' + esc(oldName)
            + ' knew about them (tags, read status) is forgotten until ' + esc(newName) + ' sends its own.';
    }

    // ---- pages ----

    // What Home says for each state of the service (ADR 0008, 0020).
    var STATE_LINES = {
        connected: "Connected",
        searching: "Connecting…",
        not_found: "Not connected",
        password_rejected: "Calibre rejected the password",
        waiting_for_wifi: "Waiting for Wi-Fi",
        paused: "Disconnected",
        ejected: "Disconnected by Calibre",
        locked_out: "Calibre isn't accepting this Kindle",
        error: "Problem",
        not_linked: "No library linked",
        stopped: "Stopped"
    };

    function readSyncText(lib) {
        if (!lib.writable) return "Not syncing reading status";
        if (!lib.read_column && !lib.last_read_column) return "Reading status sync is on, but no columns are chosen";
        return "Syncing reading status to Calibre";
    }

    var PAGES = {};

    PAGES.home = function (s) {
        var lib = s.library;
        // One button: Disconnect while the service is trying, Connect
        // otherwise (resume, or try again now).
        var trying = !s.paused && (s.state === "connected" || s.state === "searching" || s.state === "waiting_for_wifi");
        var html = header("Wireless Sync", null, true)
            + '<div class="card">'
            + fact("Status", '<b>' + esc(STATE_LINES[s.state] || s.state) + '</b>')
            + fact("Linked to", esc(lib.name || lib.uuid))
            + fact("Address", esc(where(lib)))
            + '<div class="note">' + esc(readSyncText(lib)) + '</div>';
        if (s.state === "password_rejected") {
            html += wide("Enter the password", "password", { cls: "btn-strong", arg: "home" });
        } else if (s.state !== "connected" && s.state !== "paused" && s.detail) {
            html += '<div class="hint">' + esc(s.detail) + '</div>';
        }
        html += '</div>'
            + btnRow([button(trying ? "Disconnect" : "Connect", trying ? "disconnect" : "connect", { cls: "btn-strong" }),
                      button("Library settings", "go", { arg: "settings" })])
            + btnRow([button("Change library", "change"), button("Forget library", "unlink")]);
        return html;
    };

    PAGES.choose = function (s) {
        var html = s.library ? header("Choose a library", "back:home")
            : header("Wireless Sync", null, true) + '<h1>Choose a Calibre library</h1>';
        html += '<div class="hint">In Calibre, choose <b>Connect/share → Start wireless device connection</b>. '
            + 'Then tap the library to use.</div>'
            + sectionLabel("Found on the network", finding ? "Searching…" : "");
        var found = s.found || [];
        if (s.wifi === false) {
            html += '<div class="warning"><b>Wi-Fi is off.</b> Turn it on to find Calibre libraries.</div>';
        } else if (!found.length) {
            html += '<div class="empty">' + (finding ? "Looking for Calibre…" : esc(findNote || "No libraries found.")) + '</div>';
        } else {
            html += '<div class="rows">';
            for (var i = 0; i < found.length; i++) {
                var f = found[i];
                html += '<div class="row" data-act="probe" data-arg="' + esc(f.uuid) + '">'
                    + '<div class="name">' + esc(f.name || f.uuid) + '</div><div class="sub">' + esc(where(f)) + '</div>'
                    + '<span class="chev">›</span></div>';
            }
            html += '</div>';
            if (findNote) html += '<div class="hint">' + esc(findNote) + '</div>';
        }
        html += btnRow([button("Search again", "find", { disabled: finding || s.wifi === false }),
                        button("Connect by address…", "go", { arg: "address" })]);
        if (s.library) html += '<div class="hint">Linking another library replaces ' + esc(s.library.name) + '.</div>';
        return html;
    };

    PAGES.address = function () {
        return header("Connect by address", "back:" + addressFrom)
            + '<div class="hint">For when Find can\'t see your computer. In Calibre\'s wireless settings, '
            + 'tick <b>Use fixed network port</b> so the port doesn\'t change.</div>'
            + field("inHost", "Computer name or address", "text", addr.host)
            + field("inPort", "Port", "number", addr.port)
            + field("inPassword", "Password (if Calibre has one)", "password", addr.password)
            + wide("Connect", "checkaddress", { cls: "btn-strong" });
    };

    PAGES.probe = function (s) {
        var p = s.probe || { state: "checking", target: {} };
        var name = p.target.name || address(p.target);
        if (p.state === "checking") {
            return '<h1 class="top">Testing connection to ' + esc(name) + '…</h1>'
                + '<div class="hint">Calibre will show the Kindle as connected for a few seconds.</div>'
                + wide("Cancel", "cancelprobe");
        }
        if (p.state === "password_needed" || p.state === "password_rejected") {
            return header(name, "cancelprobe")
                + '<h1>' + (p.state === "password_needed" ? "Password needed" : "Wrong password") + '</h1>'
                + '<div class="detail">' + (p.state === "password_needed"
                    ? esc(name) + ' asks for a password.'
                    : 'Calibre rejected that password.') + '</div>'
                + '<div class="hint">It\'s set in Calibre\'s <b>Start wireless device connection</b> window.</div>'
                + field("inProbePw", "Password", "password", "")
                + wide("Continue", "probepw", { cls: "btn-strong" });
        }
        return header(name, "cancelprobe")
            + '<h1>Couldn\'t connect to ' + esc(name) + '</h1>'
            + '<div class="detail">' + esc(p.error || "Something went wrong.") + '</div>'
            + wide("Try again", "retryprobe", { cls: "btn-strong" });
    };

    function pickerRow(label, kind, columns) {
        var lookup = kind === "bool" ? form.read_column : form.last_read_column;
        var none = !columnList(columns, kind).length;
        return '<div class="row" data-act="picker" data-arg="' + kind + '">'
            + '<span class="name">' + esc(label) + '</span>'
            + '<span class="value">' + esc(none && !lookup ? "No columns yet" : columnName(columns, lookup)) + '</span>'
            + '<span class="chev">›</span></div>';
    }

    function readSyncForm(columns) {
        var html = toggleRow("Update read status in Calibre", form.writable, "writable", true)
            + '<div class="hint">When enabled, books\' completion and last-read date are synced to Calibre.</div>';
        if (!form.writable) return html;
        return html + '<div class="rows">'
            + pickerRow("Read", "bool", columns) + pickerRow("Last read", "datetime", columns) + '</div>';
    }

    PAGES.setup = function (s) {
        var lib = s.probe.library;
        return header("Link " + (lib.name || lib.uuid), "cancelprobe")
            + '<div class="sub">' + esc(where(lib)) + '</div>'
            + readSyncForm(form.columns)
            + wide("Link", "link", { cls: "btn-strong" });
    };

    PAGES.picker = function (s) {
        var setup = form.ctx === "setup";
        var columns = setup ? form.columns : s.library.columns;
        var libName = setup ? form.name : s.library.name;
        var rechecking = setup && s.probe && s.probe.state === "checking";
        var isRead = pickerKind === "bool";
        var current = isRead ? form.read_column : form.last_read_column;
        var list = columnList(columns, pickerKind);
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
        }
        // Calibre lists a new column only after a restart, which also stops
        // its wireless connection unless that starts automatically.
        if (setup) {
            html += '<div class="hint">Added a column? Calibre lists it only after a restart. Restart Calibre, '
                + 'start its wireless device connection again, then tap Refresh.</div>'
                + wide(rechecking ? "Refreshing…" : "Refresh", "recheck", { disabled: rechecking });
        } else {
            html += '<div class="hint">Added a column? Calibre lists it only after a restart. Restart Calibre, '
                + 'start its wireless device connection again, and tap Connect on the first page. '
                + 'The column then shows up here.</div>';
        }
        return html;
    };

    PAGES.settings = function (s) {
        var lib = s.library;
        var changed = form.writable !== lib.writable || form.read_column !== (lib.read_column || null)
            || form.last_read_column !== (lib.last_read_column || null);
        return header(lib.name || lib.uuid, "back:home")
            + fact("Computer", esc(lib.hostname || "–"))
            + fact("Address", esc(address(lib)))
            + fact("Password", (lib.has_password ? "Set" : "None")
                + button("Change", "password", { cls: "btn-small", arg: "settings" }))
            + readSyncForm(lib.columns)
            + (form.writable ? '<div class="hint">Columns as of the last connection to Calibre.</div>' : '')
            + wide("Save", "save", { cls: "btn-strong", disabled: !changed });
    };

    PAGES.password = function (s) {
        return header("Password", "back:" + passwordFrom)
            + '<div class="detail">The password for ' + esc(s.library.name) + ', as set in Calibre\'s '
            + '<b>Start wireless device connection</b> window.</div>'
            + field("inPw", "Password", "password", "")
            + btnRow([button("Save", "savepw", { cls: "btn-strong" }), button("No password", "clearpw")]);
    };

    PAGES.app = function (s) {
        var html = header("Settings", "back:" + appFrom)
            + toggleRow("Start automatically", s.autostart, "autostart")
            + '<div class="hint">Start Wireless Sync when the Kindle starts.</div>';
        if (s.autostart_disabled_by_guard && !s.autostart) {
            html += '<div class="warning">Autostart was turned off after the service failed to start '
                + 'properly three times in a row. Turn it on again to retry.</div>';
        }
        html += toggleRow("Group books into series", s.series, "series")
            + '<div class="hint">Books in a Calibre series show as one series in the Kindle\'s library.</div>'
            + sectionLabel("Service")
            + btnRow([button("Restart", "service", { arg: "restart" }), button("Stop", "service", { arg: "stop" })])
            + '<div class="hint">The service will start again next time you open this app'
            + (s.autostart ? ' or restart the Kindle.' : '.') + '</div>'
            + '<div class="footer">Wireless Sync ' + esc(s.version || "") + '</div>';
        return html;
    };

    PAGES.stopped = function (s) {
        return '<h1 class="top">Service not running</h1><div class="detail">' + esc(s.error)
            + ' Close and reopen Wireless Sync to start it.</div>';
    };

    // ---- dialogs ----

    function renderDialog() {
        var d = el("dialog");
        if (!dialog) { d.className = "overlay hidden"; d.innerHTML = ""; return; }
        var html = '<div class="dialog">';
        if (dialog.kind === "unlink") {
            html += '<div class="title">Forget ' + esc(dialog.name) + '?</div>'
                + '<div class="detail">Your books will remain on the Kindle, and you can reconnect to this '
                + 'library later.</div>'
                + btnRow([button("Cancel", "closedialog"), button("Forget", "dounlink", { cls: "btn-strong" })]);
        } else if (dialog.kind === "replace") {
            html += '<div class="title">Replace ' + esc(dialog.linked) + '?</div>'
                + '<div class="detail">This Kindle is linked to <b>' + esc(dialog.linked) + '</b>. Linking <b>'
                + esc(dialog.name) + '</b> replaces it. ' + forgetText(dialog.linked, dialog.name) + '</div>'
                + btnRow([button("Cancel", "closedialog"), button("Replace", "doreplace", { cls: "btn-strong" })]);
        }
        d.innerHTML = html + '</div>';
        d.className = "overlay";
    }

    // ---- navigation and rendering ----

    function settingsForm(lib) {
        return { ctx: "settings", writable: !!lib.writable, read_column: lib.read_column || null,
                 last_read_column: lib.last_read_column || null };
    }

    function setupForm(lib, old) {
        if (old && old.ctx === "setup" && old.uuid === lib.uuid) {
            old.columns = lib.columns || {};
            return old;
        }
        var cols = lib.columns || {};
        return { ctx: "setup", uuid: lib.uuid, name: lib.name || lib.uuid, columns: cols, writable: false,
                 read_column: hasColumn(cols, "bool", "#kindle_read") ? "#kindle_read" : null,
                 last_read_column: hasColumn(cols, "datetime", "#kindle_last_read") ? "#kindle_last_read" : null };
    }

    function go(to) {
        if (to === "root") to = rootPage();
        // Back from a column picker keeps the edits; otherwise start from the saved settings.
        if (to === "settings" && status.library && page !== "picker") form = settingsForm(status.library);
        if (to === "choose") startFind();
        if (to === "address" && page !== "probe") addressFrom = page;
        page = to;
        redraw();
    }

    function fixPage(s) {
        if (s.error) { page = "stopped"; return; }
        if (page === "stopped" || !page) {
            page = rootPage();
            if (page === "choose") startFind();
        }
        if (!s.library && (page === "home" || page === "settings" || page === "password")) {
            page = rootPage();
            startFind();
        }
        if (s.probe) probeStarting = false;
        var setupPicker = page === "picker" && form && form.ctx === "setup";
        var probing = page === "probe" || page === "setup" || setupPicker;
        if (probing && !s.probe && !probeStarting) page = probeFrom;
        if (page === "probe" && s.probe && s.probe.state === "ok") {
            form = setupForm(s.probe.library, form);
            page = "setup";
        }
        // Refresh from a picker stays there, with the columns it found;
        // a problem (a changed password, say) shows on the check's page.
        if (setupPicker && s.probe) {
            if (s.probe.state === "ok") form = setupForm(s.probe.library, form);
            else if (s.probe.state !== "checking") page = "probe";
        }
        if (page === "picker" && !form) page = rootPage();
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
            renderedKey = key;
        }
        renderDialog();
        if (status.config_error) showMessage("Settings file problem: " + status.config_error);
    }

    function redraw() { renderedKey = null; render(); }

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

    /// "Find libraries": the reply comes once the search is done, and the
    /// found libraries arrive in /status.
    function startFind() {
        if (finding) return;
        finding = true;
        findNote = "";
        request("POST", "/find", {}, function (data, err) {
            finding = false;
            if (err) {
                findNote = err;
            } else {
                var notes = [], servers = data.servers || [];
                for (var i = 0; i < servers.length; i++) {
                    if (servers[i].status === "error") notes.push(servers[i].hostname + ": " + servers[i].error);
                }
                findNote = notes.length ? notes.join(" ") : (servers.length ? "" : "No Calibre found. Check that "
                    + "its wireless device connection is started and the Kindle is on the same network.");
            }
            lastStatusText = null;
            refresh();
        });
        if (status) redraw();
    }

    // ---- actions ----

    function startProbe(body, from, stay) {
        lastProbe = body;
        probeFrom = from;
        probeStarting = true;
        if (!stay) page = "probe";
        post("/probe", body);
        redraw();
    }

    function doLink(replace) {
        var name = form.name;
        post("/link", { writable: form.writable, read_column: form.writable ? form.read_column : null,
                        last_read_column: form.writable ? form.last_read_column : null, replace: !!replace },
            function (data) {
                if (data && data.needs_replace) {
                    dialog = { kind: "replace", linked: data.linked.name || data.linked.uuid,
                               name: data.library.name || data.library.uuid };
                    renderDialog();
                } else if (data && data.ok) {
                    showMessage("Linked to " + name + ".");
                    form = null;
                    page = "home";
                }
            });
    }

    var ACTIONS = {
        go: function (arg) { go(arg); },
        appsettings: function () { appFrom = page; page = "app"; redraw(); },
        autostart: function () {
            var on = !status.autostart;
            post("/autostart", { enabled: on }, function (data) {
                if (data && data.ok) showMessage(on ? "Will start automatically." : "Won't start automatically.");
            });
        },
        series: function () { post("/series", { enabled: !status.series }); },
        connect: function () { post(status.paused ? "/resume" : "/connect-now"); },
        disconnect: function () { post("/pause"); },
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
        recheck: function () { startProbe(lastProbe, probeFrom, true); },
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
        // Replacing another library asks first, in a dialog.
        link: function () { doLink(false); },
        doreplace: function () { dialog = null; renderDialog(); doLink(true); },
        closedialog: function () { dialog = null; renderDialog(); },
        save: function () {
            post("/settings", { writable: form.writable, read_column: form.read_column,
                                last_read_column: form.last_read_column }, function (data) {
                if (data && data.ok) { showMessage("Saved. Reconnecting to Calibre to use them."); page = "home"; }
            });
        },
        password: function (from) { passwordFrom = from; page = "password"; redraw(); },
        savepw: function () {
            post("/password", { uuid: status.library.uuid, password: el("inPw").value || null }, function (data) {
                if (data && data.ok) { showMessage("Password saved."); page = passwordFrom; redraw(); }
            });
        },
        clearpw: function () {
            post("/password", { uuid: status.library.uuid, password: null }, function (data) {
                if (data && data.ok) { showMessage("Password cleared."); page = passwordFrom; redraw(); }
            });
        },
        unlink: function () { dialog = { kind: "unlink", name: status.library.name }; renderDialog(); },
        dounlink: function () {
            dialog = null;
            renderDialog();
            post("/unlink", { uuid: status.library.uuid }, function (data) {
                if (data && data.ok) showMessage("Library forgotten. Your books stay on the Kindle.");
            });
        },
        service: function (what) {
            post("/service", { action: what }, function (data) {
                if (data && data.ok) showMessage(what === "stop" ? "Stopping the service." : "Restarting the service.");
            });
        }
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

    // On the on-screen keyboard, Enter moves to the next field on the page;
    // on the last one it closes the keyboard so the buttons show again.
    function onKeyDown(e) {
        var t = e.target;
        if (e.keyCode !== 13 || !t || t.tagName !== "INPUT") return;
        e.preventDefault();
        var inputs = el("page").getElementsByTagName("input");
        for (var i = 0; i < inputs.length - 1; i++) {
            if (inputs[i] === t) { inputs[i + 1].focus(); return; }
        }
        t.blur();
    }

    function init() {
        document.addEventListener("click", onClick, false);
        document.addEventListener("keydown", onKeyDown, true);
        refresh();
        setInterval(refresh, POLL_MS);
    }

    if (document.readyState === "complete" || document.readyState === "interactive") init();
    else document.addEventListener("DOMContentLoaded", init, false);
})();
