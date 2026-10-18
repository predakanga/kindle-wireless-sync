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
    var FOLDERS_PER_PAGE = 10;

    var status = null;          // last /status, or {error} when the service is down
    var lastStatusText = null;
    var page = null;            // see PAGES
    var renderedKey = null;     // form pages aren't redrawn under the user's typing
    var form = null;            // settings being edited: {ctx: "setup"|"settings", columns, ...}
    var pickerKind = null;      // "bool" or "datetime"
    var dialog = null;          // {kind: "unlink"|"replace", ...}
    var lastProbe = null;       // the /probe body, for Try again
    var probeStarting = false;  // sent /probe; its status hasn't arrived yet
    var probeFrom = "choose";   // where Back goes from a probe
    var passwordFrom = "settings";
    var appFrom = "home";       // where Back goes from the app settings
    var addressFrom = "choose"; // where Back goes from Connect by address
    var addr = { host: "", port: "9090", password: "" };
    var browse = null;          // the folder picker: {path, folders, at, loading} (ADR 0023)
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

    /// A books folder as the user sees it: "documents" or "documents/…".
    function folderLabel(path) { return "documents" + (path ? "/" + path : ""); }

    function parentOf(path) {
        var i = path.lastIndexOf("/");
        return i < 0 ? "" : path.substring(0, i);
    }

    function lastPart(path) { return path ? path.substring(path.lastIndexOf("/") + 1) : "documents"; }

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

    /// The first page: Home once linked, otherwise the list of libraries.
    function rootPage() { return status && status.library ? "home" : "choose"; }

    // ---- building blocks ----

    // A folder outline for the folder picker (ADR 0023), drawn like the gear.
    var FOLDER = '<svg class="folder-icon" viewBox="0 0 24 24" fill="currentColor">'
        + '<path fill-rule="evenodd" d="M2 4h7.5l2 2.5H22V20H2zM4 9v9h16V9z"/></svg>';
    // And a U-turn arrow for its "Up to …" row.
    var UP = '<svg class="folder-icon" viewBox="0 0 24 24">'
        + '<path d="M5 20.5h9.5a6.25 6.25 0 0 0 0-12.5H8" fill="none" stroke="currentColor" stroke-width="3"/>'
        + '<path d="M9 2.5v11l-7-5.5z" fill="currentColor"/></svg>';

    /// A page header: a back button (backAct) or none, and a gear for the
    /// app settings on the first pages.
    function header(title, backAct, gear) {
        return '<div class="header">'
            + (backAct ? '<button class="back" data-act="' + backAct + '">‹</button>' : '')
            + '<div class="title' + (backAct ? '' : ' no-back') + (gear ? ' with-gear' : '') + '">' + esc(title) + '</div>'
            + (gear ? '<button class="gear" data-act="appsettings" aria-label="Settings">⚙</button>' : '')
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
        return '<div class="btn-row' + (buttons.length === 3 ? ' thirds' : '') + '">' + buttons.join("") + '</div>';
    }

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

    function forgetText(oldName, newName) {
        return 'Your books and their series stay on the Kindle. The rest of what ' + esc(oldName)
            + ' knew about them (tags, read status) is forgotten until ' + esc(newName) + ' sends its own.';
    }

    // ---- pages ----

    // What the library card says for each state of the service.
    var STATE_LINES = {
        connected: "Connected",
        searching: "Connecting…",
        not_found: "Library not available",
        password_rejected: "Calibre rejected the password",
        waiting_for_wifi: "Waiting for Wi-Fi",
        paused: "Disconnected",
        ejected: "Disconnected",
        disconnecting: "Disconnecting…",
        locked_out: "Calibre isn't accepting this Kindle",
        error: "Problem",
        not_linked: "No library linked",
        stopped: "Stopped"
    };

    /// " (retrying in 23s)" while the service waits to try again. Counted
    /// down here between polls, so the status itself stays unchanged.
    function retryText(s) {
        if (!s.retry_at) return "";
        var left = s.retry_at - Math.floor(new Date().getTime() / 1000);
        return left > 0 ? " (retrying in " + left + "s)" : " (retrying…)";
    }

    /// The line under Home's status, if any: the service's advice, or the
    /// detail where it adds to the state's own line. Newlines break it.
    function statusDetail(s) {
        if (s.state === "password_rejected") return "";
        if (s.advice) return s.advice;
        if (!s.retry_at && s.state === "error") return s.detail || "";
        return "";
    }

    function readSyncText(lib) {
        if (!lib.writable) return "Not syncing reading status";
        if (!lib.read_column && !lib.last_read_column) return "Reading status sync is on, but no columns have been chosen";
        return "Syncing reading status to Calibre";
    }

    function fact(k, v) {
        return '<div class="fact"><span class="k">' + esc(k) + '</span><span class="v">' + v + '</span></div>';
    }

    var PAGES = {};

    PAGES.home = function (s) {
        var lib = s.library;
        // One button: Disconnect while the service is trying, Connect otherwise
        // (resume, or try again now).
        var trying = !s.paused && (s.state === "connected" || s.state === "searching" || s.state === "waiting_for_wifi");
        var detail = statusDetail(s);
        var html = header("Wireless Sync", null, true)
            + '<div class="card">'
            + fact("Status", '<b>' + esc(STATE_LINES[s.state] || s.state)
                + '<span id="retryIn">' + esc(retryText(s)) + '</span></b>'
                + (detail ? '<div class="status-detail">' + esc(detail).replace(/\n/g, "<br />") + '</div>' : ''))
            + fact("Linked to", esc(lib.name || lib.uuid))
            + fact("Address", esc(where(lib)))
            + '<div class="note">' + esc(readSyncText(lib)) + '</div>';
        if (s.state === "password_rejected") {
            html += wide("Enter the password", "password", { cls: "btn-strong", arg: "home" });
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
            + sectionLabel("Found on the network", s.finding ? "Searching…" : "");
        var found = s.found || [];
        if (s.wifi === false) {
            html += '<div class="warning"><b>Wi-Fi is off.</b> Turn it on to find Calibre libraries.</div>';
        } else if (!found.length) {
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
        html += btnRow([button("Search again", "find", { disabled: s.finding || s.wifi === false }),
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
        var none = !(columns && columns[kind] && columns[kind].length);
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
        return header("Link " + lib.name, "cancelprobe")
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
        var changed = form.writable !== lib.writable || form.read_column !== lib.read_column
            || form.last_read_column !== lib.last_read_column;
        return header(lib.name, "back:home")
            + '<div class="fact"><span class="k">Computer</span><span class="v">' + esc(lib.hostname) + '</span></div>'
            + '<div class="fact"><span class="k">Address</span><span class="v">' + esc(address(lib)) + '</span></div>'
            + '<div class="fact"><span class="k">Password</span><span class="v">' + (lib.has_password ? "Set" : "None")
            + button("Change", "password", { cls: "btn-small", arg: "settings" }) + '</span></div>'
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

    // Where new books go (ADR 0023): a folder under documents, chosen by
    // browsing. "Up to …" goes up; Back leaves without changing anything.
    PAGES.folders = function () {
        var html = header("Storage folder", "back:app")
            + '<div class="hint">New books go into the folder shown here. Books already on the Kindle '
            + 'stay where they are.</div>'
            + '<div class="folder-here">Current folder: <b>' + esc(folderLabel(browse.path)) + '</b></div>'
            + '<div class="rows">';
        if (browse.path) {
            html += '<div class="row plain up" data-act="folderup">' + UP + '<i>Up to ' + esc(lastPart(parentOf(browse.path)))
                + '</i></div>';
        }
        var list = browse.folders || [];
        var pages = Math.max(1, Math.ceil(list.length / FOLDERS_PER_PAGE));
        if (browse.at >= pages) browse.at = pages - 1;
        var shown = list.slice(browse.at * FOLDERS_PER_PAGE, (browse.at + 1) * FOLDERS_PER_PAGE);
        for (var i = 0; i < shown.length; i++) {
            var f = shown[i];
            html += f.problem
                ? '<div class="row unusable" data-act="folderproblem" data-arg="' + esc(f.problem) + '">'
                    + '<div class="name">' + FOLDER + esc(f.name) + '</div><div class="sub">Can\'t be used</div></div>'
                : '<div class="row" data-act="folderopen" data-arg="' + esc(f.name) + '">'
                    + '<div class="name">' + FOLDER + esc(f.name) + '</div><span class="chev">›</span></div>';
        }
        html += '</div>';
        if (browse.loading) html += '<div class="empty">Loading…</div>';
        else if (!list.length) html += '<div class="hint">No folders in here.</div>';
        if (pages > 1) {
            html += btnRow([button("‹ Previous", "folderpage", { arg: -1, disabled: browse.at === 0 }),
                            button("Next ›", "folderpage", { arg: 1, disabled: browse.at >= pages - 1 })])
                + '<div class="hint">Page ' + (browse.at + 1) + ' of ' + pages + '</div>';
        }
        return html + btnRow([button("New folder", "go", { arg: "newfolder" }),
                              button("Use this folder", "usefolder", { cls: "btn-strong",
                                     disabled: browse.loading })]);
    };

    PAGES.newfolder = function () {
        return header("New folder", "back:folders")
            + '<div class="detail">A new folder inside <b>' + esc(folderLabel(browse.path)) + '</b>.</div>'
            + field("inFolder", "Name", "text", "")
            + wide("Create", "createfolder", { cls: "btn-strong" });
    };

    PAGES.app = function (s) {
        var html = header("Settings", "back:" + appFrom)
            + toggleRow("Start automatically", s.autostart, "autostart")
            + '<div class="hint">Start Wireless Sync when the Kindle starts.</div>'
            + '<div class="toggle-row"><span class="toggle-label">Storage folder</span>'
            + button("Change", "folders", { cls: "btn-small" }) + '</div>'
            + '<div class="hint">Books are currently syncing to <i>' + esc(folderLabel(s.books_folder || "")) + '</i>.</div>'
            + toggleRow("Group books into series", s.series, "series")
            + '<div class="hint">Books in a Calibre series show as one series in the Kindle\'s library.</div>';
        html += sectionLabel("Service")
            + btnRow([button("Restart", "service", { arg: "restart" }), button("Stop", "service", { arg: "stop" })])
            + '<div class="hint">The service will start again next time you open this app'
            + (s.autostart ? ' or restart the Kindle.' : '.') + '</div>';
        return html + '<div class="footer">Wireless Sync ' + esc(s.version || "") + '</div>';
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
        return { ctx: "settings", writable: lib.writable, read_column: lib.read_column,
                 last_read_column: lib.last_read_column };
    }

    function setupForm(lib, old) {
        if (old && old.ctx === "setup" && old.uuid === lib.uuid) {
            old.columns = lib.columns;
            return old;
        }
        var cols = lib.columns;
        return { ctx: "setup", uuid: lib.uuid, name: lib.name, columns: cols, writable: false,
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
            if (page === "choose") startFind();
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
        // a problem (a changed password, say) shows on the probe page.
        if (setupPicker && s.probe) {
            if (s.probe.state === "ok") form = setupForm(s.probe.library, form);
            else if (s.probe.state !== "checking") page = "probe";
        }
        if (page === "picker" && !form) page = rootPage();
    }

    var FORM_PAGES = { address: true, password: true, newfolder: true };

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
        report();
    }

    function redraw() { renderedKey = null; render(); }

    /// Tell the harness how much the page overflows the screen.
    function report() {
        if (window.parent === window) return;
        var pageEl = el("page");
        var used = pageEl.offsetTop + pageEl.offsetHeight;
        window.parent.postMessage({ kws: "metrics", page: page + (dialog ? " + " + dialog.kind + " dialog" : ""),
                                    overflow: used - window.innerHeight }, "*");
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
                    dialog = { kind: "replace", linked: data.linked.name, name: data.library.name };
                    renderDialog();
                    report();
                } else if (data && data.ok) {
                    showMessage("Linked to " + name + ".");
                    form = null;
                    page = "home";
                }
            });
    }

    /// Shows the folder at `path` in the picker; an unusable one falls back
    /// to documents.
    function openFolder(path) {
        browse.loading = true;
        redraw();
        request("POST", "/folders", { path: path }, function (data, err) {
            browse.loading = false;
            if (err || !data.ok) {
                showMessage(err || data.error);
                if (path) { openFolder(""); return; }
            } else {
                browse.path = data.path;
                browse.folders = data.folders;
                browse.at = 0;
            }
            if (page === "folders") redraw();
        });
    }

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
        autostart: function () { post("/autostart", { enabled: !status.autostart }); },
        series: function () { post("/series", { enabled: !status.series }); },
        folders: function () {
            browse = { path: status.books_folder || "", folders: [], at: 0 };
            page = "folders";
            openFolder(browse.path);
        },
        folderopen: function (name) { openFolder((browse.path ? browse.path + "/" : "") + name); },
        folderup: function () { openFolder(parentOf(browse.path)); },
        folderpage: function (step) { browse.at += parseInt(step, 10); redraw(); },
        folderproblem: function (why) { showMessage(why); },
        usefolder: function () {
            var path = browse.path;
            post("/books-folder", { path: path }, function (data) {
                if (data && data.ok) { showMessage("New books will go into " + folderLabel(path) + "."); page = "app"; }
            });
        },
        createfolder: function () {
            var name = el("inFolder").value.replace(/^\s+|\s+$/g, "");
            if (!name) { showMessage("Enter a name for the folder."); return; }
            post("/folders/new", { path: browse.path, name: name }, function (data) {
                if (data && data.ok) { page = "folders"; openFolder(data.path); }
            });
        },
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
                if (data && data.ok) showMessage("Library forgotten. Your books stay on the Kindle.");
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
    /// Moves Home's retry countdown on, redrawing only that text.
    function tick() {
        var e = el("retryIn");
        if (!e || !status || page !== "home") return;
        var text = esc(retryText(status));
        if (e.innerHTML !== text) e.innerHTML = text;
    }

    setInterval(refresh, POLL_MS);
    setInterval(tick, 1000);
})();
