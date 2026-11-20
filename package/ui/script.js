/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Wireless Sync settings app: one screen per page (ADR 0020), talking to the
 * service's localhost API (ADR 0011). ES5 only: the Kindle's mesquite engine
 * has no fetch, promises or arrow functions. The design was settled with
 * tools/ui-prototype. Its text is in strings-<language>.js (ADR 0028).
 */
(function () {
    "use strict";

    var API = "http://127.0.0.1:8325";
    var POLL_MS = 3000;
    var MESSAGE_MS = 6000;
    var FOLDERS_PER_PAGE = 10;

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
    var browse = null;          // the folder picker: {path, folders, at, loading} (ADR 0023)
    var finding = false;        // /find is running
    var findNote = "";          // what the last search couldn't reach
    var messageTimer = null;

    // ---- text (ADR 0028) ----

    // strings-en.js is loaded first; a translation adds its own table.
    var STRINGS = window.KWS_STRINGS || (window.KWS_STRINGS = {});
    var language = null;        // the translation's table, if any
    var locale = null;          // the locale its files were asked for

    function template(key) {
        if (language && language[key] !== undefined) return language[key];
        if (STRINGS.en && STRINGS.en[key] !== undefined) return STRINGS.en[key];
        return null;
    }

    /// The text for `key` as HTML, with each {name} filled from `params`,
    /// escaped. The templates themselves may hold <b> and <i>.
    function t(key, params) {
        var text = template(key);
        if (text === null) return esc(key);
        return text.replace(/\{(\w+)\}/g, function (all, name) {
            var v = params ? params[name] : undefined;
            return v === undefined || v === null ? all : esc(v);
        });
    }

    /// A message from the service as HTML: its code's text when known,
    /// otherwise the English the service sent alongside it.
    function msg(m, english) {
        if (m && m.code && template(m.code) !== null) return t(m.code, m);
        return english ? esc(english) : "";
    }

    /// Loads the translation for `loc` ("de_DE"): strings-de_DE.js and
    /// strings-de.js, the most specific one that loads winning. Nothing is
    /// waited for: a missing file just never adds its table.
    function loadLanguage(loc) {
        if (!loc || loc === locale) return;
        locale = loc;
        var names = [loc];
        var base = loc.split(/[_-]/)[0];
        if (base !== loc) names.push(base);
        function pick() {
            for (var i = 0; i < names.length; i++) {
                if (names[i] === "en") break;
                if (STRINGS[names[i]]) {
                    if (language !== STRINGS[names[i]]) { language = STRINGS[names[i]]; redraw(); }
                    return;
                }
            }
        }
        for (var i = 0; i < names.length; i++) {
            if (names[i] === "en" || STRINGS[names[i]]) continue;
            var script = document.createElement("script");
            script.src = "strings-" + names[i] + ".js";
            script.onload = pick;
            document.getElementsByTagName("head")[0].appendChild(script);
        }
        pick();
    }

    // ---- HTTP ----

    function request(method, path, body, cb) {
        var xhr = new XMLHttpRequest();
        var done = false;
        var timer = setTimeout(function () {
            if (!done) { done = true; xhr.abort(); cb(null, t("err.no_answer")); }
        }, 30000);
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4 || done) return;
            done = true;
            clearTimeout(timer);
            if (xhr.status === 0) return cb(null, t("err.not_running"));
            var data = null;
            try { data = JSON.parse(xhr.responseText); } catch (e) { /* handled below */ }
            if (!data) return cb(null, t("err.unexpected_reply", { status: xhr.status }));
            if (xhr.status !== 200) return cb(null, msg(data.message, data.error) || t("err.http", { status: xhr.status }));
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
            cb(null, t("err.unreachable"));
        }
    }

    /// POST, reporting failures ({ok: false}) as messages. Errors given to
    /// callbacks are HTML.
    function post(path, body, cb) {
        request("POST", path, body || {}, function (data, err) {
            if (err) showMessage(err);
            else if (!data.ok && !data.needs_replace) showMessage(msg(data.message, data.error) || t("err.didnt_work"));
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

    /// Shows `html` for a few seconds.
    function showMessage(html) {
        var m = el("message");
        m.innerHTML = html;
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

    /// A column's name, as HTML.
    function columnName(columns, lookup) {
        if (!lookup) return t("column.dont_sync");
        var c = columns && columns[lookup];
        return c ? esc(c.name || lookup) : t("column.missing", { lookup: lookup });
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

    // A folder outline for the folder picker (ADR 0023), drawn like the gear.
    var FOLDER = '<svg class="folder-icon" viewBox="0 0 24 24" fill="currentColor">'
        + '<path fill-rule="evenodd" d="M2 4h7.5l2 2.5H22V20H2zM4 9v9h16V9z"/></svg>';
    // And a U-turn arrow for its "Up to …" row.
    var UP = '<svg class="folder-icon" viewBox="0 0 24 24">'
        + '<path d="M5 20.5h9.5a6.25 6.25 0 0 0 0-12.5H8" fill="none" stroke="currentColor" stroke-width="3"/>'
        + '<path d="M9 2.5v11l-7-5.5z" fill="currentColor"/></svg>';

    // The building blocks below take HTML: t() for text, esc() for data.

    /// A page header: a back button (backAct) or none, and a gear for
    /// Settings on the first pages.
    function header(title, backAct, gear) {
        return '<div class="header">'
            + (backAct ? '<button class="back" data-act="' + backAct + '">‹</button>' : '')
            + '<div class="title' + (backAct ? '' : ' no-back') + (gear ? ' with-gear' : '') + '">' + title + '</div>'
            + (gear ? '<button class="gear" data-act="appsettings">' + GEAR + '</button>' : '')
            + '</div>';
    }

    function toggleRow(label, on, act, boxed) {
        return '<div class="toggle-row' + (boxed ? ' boxed' : '') + '">'
            + '<span class="toggle-label">' + label + '</span>'
            + '<button class="toggle' + (on ? ' on' : '') + '" data-act="' + act + '"><span class="knob"></span></button></div>';
    }

    function button(label, act, opts) {
        opts = opts || {};
        return '<button class="btn' + (opts.cls ? ' ' + opts.cls : '') + '" data-act="' + act + '"'
            + (opts.arg !== undefined ? ' data-arg="' + esc(opts.arg) + '"' : '')
            + (opts.disabled ? ' disabled="disabled"' : '') + '>' + label + '</button>';
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
        return '<div class="section-label">' + text
            + (aside ? '<span class="aside">' + aside + '</span>' : '') + '</div>';
    }

    function field(id, label, type, value) {
        return '<div class="field"><label for="' + id + '">' + label + '</label>'
            + '<input type="' + type + '" id="' + id + '" value="' + esc(value || "") + '" autocomplete="off" autocapitalize="off" /></div>';
    }

    function fact(k, v) {
        return '<div class="fact"><span class="k">' + k + '</span><span class="v">' + v + '</span></div>';
    }

    function forgetText(oldName, newName) {
        return t("replace.forget", { old: oldName, "new": newName });
    }

    // ---- pages ----

    /// What Home says for a state of the service (ADR 0008, 0020).
    function stateLine(state) {
        return template("state." + state) !== null ? t("state." + state) : esc(state);
    }

    /// " (retrying in 23s)" while the service waits to try again, as HTML.
    /// Counted down here between polls, so the status itself stays unchanged.
    function retryText(s) {
        if (!s.retry_at) return "";
        var left = s.retry_at - Math.floor(new Date().getTime() / 1000);
        return left > 0 ? t("home.retrying_in", { secs: left }) : t("home.retrying");
    }

    /// The line under Home's status, if any, as HTML: the service's
    /// advice, or the detail where it adds to the state's own line.
    /// Newlines break it.
    function statusDetail(s) {
        if (s.state === "password_rejected") return "";
        if (s.advice) return msg(s.advice_msg, s.advice);
        if (!s.retry_at && s.state === "error") return msg(s.detail_msg, s.detail);
        return "";
    }

    // Older firmware can't sync reading status (ADR 0026); an unknown
    // version can.
    function readSyncAvailable(s) { return s.read_sync_available !== false; }

    function readSyncText(s, lib) {
        if (!readSyncAvailable(s)) return t("readsync.unavailable");
        if (!lib.writable) return t("readsync.off");
        if (!lib.read_column && !lib.last_read_column) return t("readsync.no_columns");
        return t("readsync.on");
    }

    var PAGES = {};

    PAGES.home = function (s) {
        var lib = s.library;
        // One button: Disconnect while the service is trying, Connect
        // otherwise (resume, or try again now).
        var trying = !s.paused && (s.state === "connected" || s.state === "searching" || s.state === "waiting_for_wifi");
        var detail = statusDetail(s);
        var html = header(t("app.name"), null, true)
            + '<div class="card">'
            + fact(t("home.status"), '<b>' + stateLine(s.state)
                + '<span id="retryIn">' + retryText(s) + '</span></b>'
                + (detail ? '<div class="status-detail">' + detail.replace(/\n/g, "<br />") + '</div>' : ''))
            + fact(t("home.linked_to"), esc(lib.name || lib.uuid))
            + fact(t("common.address"), esc(where(lib)))
            + '<div class="note">' + readSyncText(s, lib) + '</div>';
        if (s.state === "password_rejected") {
            html += wide(t("home.enter_password"), "password", { cls: "btn-strong", arg: "home" });
        }
        html += '</div>'
            + btnRow([button(t(trying ? "home.disconnect" : "home.connect"), trying ? "disconnect" : "connect", { cls: "btn-strong" }),
                      button(t("home.library_settings"), "go", { arg: "settings" })])
            + btnRow([button(t("home.change_library"), "change"), button(t("home.forget_library"), "unlink")]);
        return html;
    };

    PAGES.choose = function (s) {
        var html = s.library ? header(t("choose.title"), "back:home")
            : header(t("app.name"), null, true) + '<h1>' + t("choose.heading") + '</h1>';
        html += '<div class="hint">' + t("choose.hint") + '</div>'
            + sectionLabel(t("choose.found"), finding ? t("choose.searching") : "");
        var found = s.found || [];
        if (s.wifi === false) {
            html += '<div class="warning">' + t("choose.wifi_off") + '</div>';
        } else if (!found.length) {
            html += '<div class="empty">' + (finding ? t("choose.looking") : (findNote || t("choose.none"))) + '</div>';
        } else {
            html += '<div class="rows">';
            for (var i = 0; i < found.length; i++) {
                var f = found[i];
                html += '<div class="row" data-act="probe" data-arg="' + esc(f.uuid) + '">'
                    + '<div class="name">' + esc(f.name || f.uuid) + '</div><div class="sub">' + esc(where(f)) + '</div>'
                    + '<span class="chev">›</span></div>';
            }
            html += '</div>';
            if (findNote) html += '<div class="hint">' + findNote + '</div>';
        }
        html += btnRow([button(t("choose.search_again"), "find", { disabled: finding || s.wifi === false }),
                        button(t("choose.by_address"), "go", { arg: "address" })]);
        if (s.library) html += '<div class="hint">' + t("choose.replaces", { name: s.library.name }) + '</div>';
        return html;
    };

    PAGES.address = function () {
        return header(t("address.title"), "back:" + addressFrom)
            + '<div class="hint">' + t("address.hint") + '</div>'
            + field("inHost", t("address.host"), "text", addr.host)
            + field("inPort", t("address.port"), "number", addr.port)
            + field("inPassword", t("address.password"), "password", addr.password)
            + wide(t("address.connect"), "checkaddress", { cls: "btn-strong" });
    };

    PAGES.probe = function (s) {
        var p = s.probe || { state: "checking", target: {} };
        var name = p.target.name || address(p.target);
        if (p.state === "checking") {
            return '<h1 class="top">' + t("probe.testing", { name: name }) + '</h1>'
                + '<div class="hint">' + t("probe.testing_hint") + '</div>'
                + wide(t("common.cancel"), "cancelprobe");
        }
        if (p.state === "password_needed" || p.state === "password_rejected") {
            return header(esc(name), "cancelprobe")
                + '<h1>' + t(p.state === "password_needed" ? "probe.password_needed" : "probe.wrong_password") + '</h1>'
                + '<div class="detail">' + (p.state === "password_needed"
                    ? t("probe.asks_password", { name: name })
                    : t("probe.rejected")) + '</div>'
                + '<div class="hint">' + t("probe.password_where") + '</div>'
                + field("inProbePw", t("common.password"), "password", "")
                + wide(t("probe.continue"), "probepw", { cls: "btn-strong" });
        }
        return header(esc(name), "cancelprobe")
            + '<h1>' + t("probe.failed", { name: name }) + '</h1>'
            + '<div class="detail">' + (msg(p.message, p.error) || t("err.something_wrong")) + '</div>'
            + wide(t("probe.try_again"), "retryprobe", { cls: "btn-strong" });
    };

    function pickerRow(label, kind, columns) {
        var lookup = kind === "bool" ? form.read_column : form.last_read_column;
        var none = !columnList(columns, kind).length;
        return '<div class="row" data-act="picker" data-arg="' + kind + '">'
            + '<span class="name">' + label + '</span>'
            + '<span class="value">' + (none && !lookup ? t("picker.no_columns_yet") : columnName(columns, lookup)) + '</span>'
            + '<span class="chev">›</span></div>';
    }

    function readSyncForm(s, columns) {
        if (!readSyncAvailable(s)) {
            return '<div class="hint">' + t("svc.read_sync_unavailable", { min: "5.19.1", firmware: s.firmware }) + '</div>';
        }
        var html = toggleRow(t("readsync.toggle"), form.writable, "writable", true)
            + '<div class="hint">' + t("readsync.hint") + '</div>';
        if (!form.writable) return html;
        return html + '<div class="rows">'
            + pickerRow(t("readsync.read"), "bool", columns) + pickerRow(t("readsync.last_read"), "datetime", columns) + '</div>';
    }

    PAGES.setup = function (s) {
        var lib = s.probe.library;
        return header(t("setup.title", { name: lib.name || lib.uuid }), "cancelprobe")
            + '<div class="sub">' + esc(where(lib)) + '</div>'
            + readSyncForm(s, form.columns)
            + wide(t("setup.link"), "link", { cls: "btn-strong" });
    };

    PAGES.picker = function (s) {
        var setup = form.ctx === "setup";
        var columns = setup ? form.columns : s.library.columns;
        var libName = setup ? form.name : s.library.name;
        var rechecking = setup && s.probe && s.probe.state === "checking";
        var isRead = pickerKind === "bool";
        var current = isRead ? form.read_column : form.last_read_column;
        var list = columnList(columns, pickerKind);
        var html = header(t(isRead ? "picker.read_title" : "picker.last_read_title"), "back:" + form.ctx)
            + '<div class="hint">' + t(isRead ? "picker.bool_in" : "picker.date_in", { name: libName }) + '</div>';
        html += '<div class="rows"><div class="row plain" data-act="pick" data-arg="">'
            + '<span class="mark' + (!current ? ' on' : '') + '"></span>' + t("column.dont_sync") + '</div>';
        for (var i = 0; i < list.length; i++) {
            html += '<div class="row plain" data-act="pick" data-arg="' + esc(list[i].lookup) + '">'
                + '<span class="mark' + (current === list[i].lookup ? ' on' : '') + '"></span>'
                + esc(list[i].name) + ' <span class="sub">' + esc(list[i].lookup) + '</span></div>';
        }
        html += '</div>';
        if (!list.length) {
            html += '<div class="hint">' + t(isRead ? "picker.bool_none" : "picker.date_none", { name: libName }) + '</div>';
        }
        // Calibre lists a new column only after a restart, which also stops
        // its wireless connection unless that starts automatically.
        if (setup) {
            html += '<div class="hint">' + t("picker.restart_setup") + '</div>'
                + wide(t(rechecking ? "picker.refreshing" : "picker.refresh"), "recheck", { disabled: rechecking });
        } else {
            html += '<div class="hint">' + t("picker.restart_settings") + '</div>';
        }
        return html;
    };

    PAGES.settings = function (s) {
        var lib = s.library;
        var changed = form.writable !== lib.writable || form.read_column !== (lib.read_column || null)
            || form.last_read_column !== (lib.last_read_column || null);
        return header(esc(lib.name || lib.uuid), "back:home")
            + fact(t("settings.computer"), esc(lib.hostname || "–"))
            + fact(t("common.address"), esc(address(lib)))
            + fact(t("common.password"), t(lib.has_password ? "settings.password_set" : "settings.password_none")
                + button(t("common.change"), "password", { cls: "btn-small", arg: "settings" }))
            + readSyncForm(s, lib.columns)
            + (form.writable && readSyncAvailable(s) ? '<div class="hint">' + t("settings.columns_as_of") + '</div>' : '')
            + wide(t("common.save"), "save", { cls: "btn-strong", disabled: !changed });
    };

    PAGES.password = function (s) {
        return header(t("common.password"), "back:" + passwordFrom)
            + '<div class="detail">' + t("password.detail", { name: s.library.name }) + '</div>'
            + field("inPw", t("common.password"), "password", "")
            + btnRow([button(t("common.save"), "savepw", { cls: "btn-strong" }), button(t("password.none"), "clearpw")]);
    };

    // Where new books go (ADR 0023): a folder under documents, chosen by
    // browsing. "Up to …" goes up; Back leaves without changing anything.
    PAGES.folders = function () {
        var html = header(t("folders.title"), "back:app")
            + '<div class="hint">' + t("folders.hint") + '</div>'
            + '<div class="folder-here">' + t("folders.current", { folder: folderLabel(browse.path) }) + '</div>'
            + '<div class="rows">';
        if (browse.path) {
            html += '<div class="row plain up" data-act="folderup">' + UP + '<i>'
                + t("folders.up", { folder: lastPart(parentOf(browse.path)) }) + '</i></div>';
        }
        var list = browse.folders || [];
        var pages = Math.max(1, Math.ceil(list.length / FOLDERS_PER_PAGE));
        if (browse.at >= pages) browse.at = pages - 1;
        var shown = list.slice(browse.at * FOLDERS_PER_PAGE, (browse.at + 1) * FOLDERS_PER_PAGE);
        for (var i = 0; i < shown.length; i++) {
            var f = shown[i];
            html += f.problem
                ? '<div class="row unusable" data-act="folderproblem" data-arg="' + esc(msg(f.problem_msg, f.problem)) + '">'
                    + '<div class="name">' + FOLDER + esc(f.name) + '</div><div class="sub">' + t("folders.unusable") + '</div></div>'
                : '<div class="row" data-act="folderopen" data-arg="' + esc(f.name) + '">'
                    + '<div class="name">' + FOLDER + esc(f.name) + '</div><span class="chev">›</span></div>';
        }
        html += '</div>';
        if (browse.loading) html += '<div class="empty">' + t("folders.loading") + '</div>';
        else if (!list.length) html += '<div class="hint">' + t("folders.empty") + '</div>';
        if (pages > 1) {
            html += btnRow([button(t("folders.previous"), "folderpage", { arg: -1, disabled: browse.at === 0 }),
                            button(t("folders.next"), "folderpage", { arg: 1, disabled: browse.at >= pages - 1 })])
                + '<div class="hint">' + t("folders.page", { page: browse.at + 1, pages: pages }) + '</div>';
        }
        return html + btnRow([button(t("folders.new"), "go", { arg: "newfolder" }),
                              button(t("folders.use"), "usefolder", { cls: "btn-strong",
                                     disabled: browse.loading })]);
    };

    PAGES.newfolder = function () {
        return header(t("newfolder.title"), "back:folders")
            + '<div class="detail">' + t("newfolder.inside", { folder: folderLabel(browse.path) }) + '</div>'
            + field("inFolder", t("newfolder.name"), "text", "")
            + wide(t("newfolder.create"), "createfolder", { cls: "btn-strong" });
    };

    PAGES.app = function (s) {
        var html = header(t("app.settings"), "back:" + appFrom)
            + toggleRow(t("app.autostart"), s.autostart, "autostart")
            + '<div class="hint">' + t("app.autostart_hint") + '</div>';
        if (s.autostart_disabled_by_guard && !s.autostart) {
            html += '<div class="warning">' + t("app.autostart_guard") + '</div>';
        }
        html += '<div class="toggle-row"><span class="toggle-label">' + t("app.storage") + '</span>'
            + button(t("common.change"), "folders", { cls: "btn-small" }) + '</div>'
            + '<div class="hint">' + t("app.syncing_to", { folder: folderLabel(s.books_folder || "") }) + '</div>'
            + toggleRow(t("app.series"), s.series, "series")
            + '<div class="hint">' + t("app.series_hint") + '</div>'
            + sectionLabel(t("app.service"))
            + btnRow([button(t("app.restart"), "service", { arg: "restart" }), button(t("app.stop"), "service", { arg: "stop" })])
            + '<div class="hint">' + t(s.autostart ? "app.starts_on_open_or_restart" : "app.starts_on_open") + '</div>'
            + '<div class="footer">' + t("app.version", { version: s.version || "" }) + '</div>';
        return html;
    };

    PAGES.stopped = function (s) {
        return '<h1 class="top">' + t("stopped.title") + '</h1><div class="detail">' + s.error
            + ' ' + t("stopped.reopen") + '</div>';
    };

    // ---- dialogs ----

    function renderDialog() {
        var d = el("dialog");
        if (!dialog) { d.className = "overlay hidden"; d.innerHTML = ""; return; }
        var html = '<div class="dialog">';
        if (dialog.kind === "unlink") {
            html += '<div class="title">' + t("unlink.title", { name: dialog.name }) + '</div>'
                + '<div class="detail">' + t("unlink.detail") + '</div>'
                + btnRow([button(t("common.cancel"), "closedialog"), button(t("unlink.forget"), "dounlink", { cls: "btn-strong" })]);
        } else if (dialog.kind === "replace") {
            html += '<div class="title">' + t("replace.title", { linked: dialog.linked }) + '</div>'
                + '<div class="detail">' + t("replace.detail", { linked: dialog.linked, name: dialog.name })
                + ' ' + forgetText(dialog.linked, dialog.name) + '</div>'
                + btnRow([button(t("common.cancel"), "closedialog"), button(t("replace.replace"), "doreplace", { cls: "btn-strong" })]);
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
        if (status.config_error) showMessage(t("err.config_problem", { error: status.config_error }));
    }

    function redraw() { renderedKey = null; render(); }

    function refresh() {
        request("GET", "/status", null, function (data, err) {
            var next = err ? { error: err } : data;
            if (data && data.locale) loadLanguage(data.locale);
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
            // findNote is HTML.
            if (err) {
                findNote = err;
            } else {
                var notes = [], servers = data.servers || [];
                for (var i = 0; i < servers.length; i++) {
                    if (servers[i].status === "error") {
                        notes.push(esc(servers[i].hostname) + ": " + msg(servers[i].message, servers[i].error));
                    }
                }
                findNote = notes.length ? notes.join(" ") : (servers.length ? "" : t("choose.not_found"));
            }
            lastStatusText = null;
            refresh();
        });
        if (status) redraw();
    }

    // ---- actions ----

    /// Shows the folder at `path` in the picker; an unusable one falls back
    /// to documents.
    function openFolder(path) {
        browse.loading = true;
        redraw();
        request("POST", "/folders", { path: path }, function (data, err) {
            browse.loading = false;
            if (err || !data.ok) {
                showMessage(err || msg(data.message, data.error));
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
                    showMessage(t("msg.linked", { name: name }));
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
                if (data && data.ok) showMessage(t(on ? "msg.autostart_on" : "msg.autostart_off"));
            });
        },
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
                if (data && data.ok) { showMessage(t("msg.folder_set", { folder: folderLabel(path) })); page = "app"; }
            });
        },
        createfolder: function () {
            var name = el("inFolder").value.replace(/^\s+|\s+$/g, "");
            if (!name) { showMessage(t("svc.folder_name_missing")); return; }
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
            if (!addr.host) { showMessage(t("svc.enter_host")); return; }
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
                if (data && data.ok) { showMessage(t("msg.saved")); page = "home"; }
            });
        },
        password: function (from) { passwordFrom = from; page = "password"; redraw(); },
        savepw: function () {
            post("/password", { uuid: status.library.uuid, password: el("inPw").value || null }, function (data) {
                if (data && data.ok) { showMessage(t("msg.password_saved")); page = passwordFrom; redraw(); }
            });
        },
        clearpw: function () {
            post("/password", { uuid: status.library.uuid, password: null }, function (data) {
                if (data && data.ok) { showMessage(t("msg.password_cleared")); page = passwordFrom; redraw(); }
            });
        },
        unlink: function () { dialog = { kind: "unlink", name: status.library.name }; renderDialog(); },
        dounlink: function () {
            dialog = null;
            renderDialog();
            post("/unlink", { uuid: status.library.uuid }, function (data) {
                if (data && data.ok) showMessage(t("msg.forgotten"));
            });
        },
        service: function (what) {
            post("/service", { action: what }, function (data) {
                if (data && data.ok) showMessage(t(what === "stop" ? "msg.stopping" : "msg.restarting"));
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

    /// Moves Home's retry countdown on, redrawing only that text.
    function tick() {
        var e = el("retryIn");
        if (!e || !status || page !== "home") return;
        var html = retryText(status);
        if (e.innerHTML !== html) e.innerHTML = html;
    }

    function init() {
        document.addEventListener("click", onClick, false);
        document.addEventListener("keydown", onKeyDown, true);
        refresh();
        setInterval(refresh, POLL_MS);
        setInterval(tick, 1000);
    }

    if (document.readyState === "complete" || document.readyState === "interactive") init();
    else document.addEventListener("DOMContentLoaded", init, false);
})();
