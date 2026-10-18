# SPDX-License-Identifier: GPL-3.0-or-later
"""The compatibility checks, run in order against a booted, jailbroken
guest. Each looks at one thing the package or app relies on; the research
notes and ADRs say why each matters."""

import json
import re
import shlex
import time
import uuid

import fitcheck
from common import ERROR, FAIL, PASS, SKIP, WARN, CheckFailed, log, result

PKG = "kindle-wireless-sync"
DIR = "/var/local/kindle-wireless-sync"
BIN = f"{DIR}/kindle-wireless-sync"
APP_ID = "com.kindlewirelesssync.app"
DOCS = "/mnt/us/documents"
CC = "/var/local/cc.db"
APPREG = "/var/local/appreg.db"
KPM = "/var/local/kmc/bin/kpm"
API = "http://127.0.0.1:8325"
# Test files are TXT and named kws-* (CLAUDE.md, "Device access"), with a
# per-run suffix: the scanner can miss a folder deleted and made again at
# once, as reruns on one disk (--attach) would.
RUN = uuid.uuid4().hex[:6]
TOP_BOOK = f"{DOCS}/kws-compat-{RUN}.txt"
SUB_DIR = f"{DOCS}/kws-compat-{RUN}-sub"
SUB_BOOK = f"{SUB_DIR}/kws-compat-{RUN}-sub.txt"

# What the package's scripts and the binary run (package/*.sh, src/).
COMMANDS = ["sh", "sqlite3", "lipc-get-prop", "lipc-set-prop", "lipc-wait-event", "/sbin/initctl",
            "/sbin/start", "/usr/sbin/mntroot", "/usr/bin/mesquite", "pgrep", "cmp", "cp", "mv", "chmod",
            "mkdir", "rm", "grep", "sleep", "date", "touch", "dirname", "cat"]
# Only the dev workflow and this harness use these.
DEV_COMMANDS = ["curl", "lipc-hash-prop", "/usr/sbin/screenshot", "tar", "timeout"]

# Catalog columns read by src/ (series.rs, kindle.rs, readsync.rs).
ENTRIES_COLUMNS = ["p_uuid", "p_location", "p_cdeKey", "p_cdeType", "p_type", "p_thumbnail", "p_seriesState",
                   "p_originType", "p_titles_0_nominal", "p_memberCount", "p_homeMemberCount", "p_isProcessed",
                   "p_virtualCollectionCount", "j_members", "p_readState", "p_lastOpenTime"]
SERIES_COLUMNS = ["d_seriesId", "d_itemCdeKey", "d_itemPosition", "d_itemPositionLabel"]
# Written by package/register-app.sh.
APPREG_COLUMNS = {"interfaces": ["interface"], "handlerIds": ["handlerId"],
                  "associations": ["handlerId", "interface", "contentId", "defaultAssoc"],
                  "properties": ["handlerId", "name", "value"]}


class Context:
    def __init__(self, guest, emu, repo, kpkg, version, artifacts, work):
        self.guest = guest
        self.emu = emu
        self.repo = repo
        self.kpkg = kpkg
        self.version = version
        self.artifacts = artifacts
        self.work = work
        self.state = {}  # what earlier checks found, for later ones
        self.extra = []  # results a check adds besides its own (the UI checks)

    def screenshot(self, name):
        path = self.artifacts / name
        try:
            p = self.emu.qmp("screendump", str(path), timeout=30)
        except Exception as e:  # noqa: BLE001 - a missing screenshot shouldn't fail a check
            log(f"screenshot {name} failed: {e}")
            return None
        if p.returncode != 0:
            log(f"screenshot {name} failed: {p.stderr.strip()}")
            return None
        return str(path)

    def needs(self, key, why):
        if not self.state.get(key):
            raise CheckFailed(SKIP, why)


CHECKS = []


def check(check_id, title, required=True):
    def wrap(fn):
        CHECKS.append((check_id, title, required, fn))
        return fn
    return wrap


def change_request(ctx, commands, attempts=10):
    """POSTs a ChangeRequest to the catalog (ADR 0016), retrying while the
    database is locked, as src/catalog.rs does."""
    body = json.dumps({"type": "ChangeRequest", "id": 1, "commands": commands})
    reply = ""
    for _ in range(attempts):
        rc, out, err = ctx.guest.run(
            "curl -s -m 30 -X POST -H \"AuthToken: $(cat /tmp/session_token)\" "
            f"--data-binary @- http://127.0.0.1:9101/change", stdin=body.encode(), timeout=60)
        reply = out.strip()
        if "database is locked" in reply:
            time.sleep(1)
            continue
        try:
            return json.loads(reply)
        except ValueError:
            break
    raise CheckFailed(FAIL, f"the catalog refused a ChangeRequest: {reply or err.strip()}")


def entry(ctx, location, fields="p_uuid, p_cdeKey, p_cdeType"):
    out = ctx.guest.sql(CC, f"SELECT {fields} FROM Entries WHERE p_location = '{location}'")
    return out.split("|") if out else None


def scanner_errors(ctx):
    """How many catalog posts the scanner has had refused so far (seen on
    5.16.2.1.1 after a KPM install: "SQL logic error" from change.lua)."""
    out = ctx.guest.run("grep -c 'sps:parserr' /var/log/messages")[1].strip()
    return int(out) if out.isdigit() else None


def api_status(ctx):
    rc, out, _ = ctx.guest.run(f"curl -s -m 10 {API}/status", timeout=30)
    try:
        return json.loads(out)
    except ValueError:
        return None


# ---- the device ----

@check("device_info", "Device and firmware details", required=False)
def device_info(ctx):
    g = ctx.guest
    info = {
        "version_txt": g.run("cat /etc/version.txt")[1].strip(),
        "prettyversion": g.run("cat /etc/prettyversion.txt 2>/dev/null")[1].strip() or None,
        "kernel": g.run("uname -rm")[1].strip(),
        "screen": g.run("cat /sys/class/graphics/fb0/modes 2>/dev/null | head -1")[1].strip() or None,
        "userstore": g.run("mount | grep ' /mnt/us '")[1].strip(),
        "free_kb": {p: g.run(f"df -k {p} | tail -1 | awk '{{print $4}}'")[1].strip()
                    for p in ("/mnt/us", "/var/local", "/")},
    }
    ctx.state["device"] = info
    return PASS, f"{info['version_txt'].splitlines()[0] if info['version_txt'] else '?'}; {info['kernel']}; " \
                 f"screen {info['screen']}", info


@check("arch_kindlehf", "The device is a kindlehf platform (the package's only supported_platforms)")
def arch_kindlehf(ctx):
    g = ctx.guest
    hf = g.ok("[ -e /lib/ld-linux-armhf.so.3 ]")
    machine = g.run("uname -m")[1].strip()
    # KPM's platform is compiled in (KPM_PLATFORM in its install.c); jb.sh
    # installs the build that matches the device.
    kpm_platform = None
    if g.ok(f"[ -x {KPM} ]"):
        # jb.sh links /var/local/kmc/bin to the kindlehf or kindlepw2 build.
        link = g.run("readlink /var/local/kmc/bin")[1].strip().rstrip("/")
        found = link.split("/")[-2] if link.endswith("/bin") and link.count("/") >= 2 else ""
        if found not in ("kindlehf", "kindlepw2"):
            found = g.run(f"strings {KPM} 2>/dev/null | grep -xE 'kindlehf|kindlepw2' | head -1")[1].strip()
        kpm_platform = found or None
    data = {"armhf_loader": hf, "uname_m": machine, "kpm_platform": kpm_platform}
    # The binary is static, so it may well run anyway; but the manifest
    # offers the package to kindlehf only.
    if not hf:
        return FAIL, (f"no /lib/ld-linux-armhf.so.3: a soft-float (kindlepw2) device ({machine}); "
                      f"KPM here is the {kpm_platform or 'unknown'} build"), data
    if kpm_platform and kpm_platform != "kindlehf":
        return FAIL, f"armhf userspace, but KPM is the {kpm_platform} build; the manifest lists only kindlehf", data
    return PASS, f"armhf userspace ({machine}); KPM platform {kpm_platform or 'unknown (no KPM)'}", data


@check("jailbreak", "Jailbroken, with KPM")
def jailbreak(ctx):
    g = ctx.guest
    kpm = g.ok(f"[ -x {KPM} ]")
    data = {"kpm": kpm, "jailbroken_txt": g.ok(f"[ -f {DOCS}/JAILBROKEN.txt ]"),
            "su": g.ok("[ -x /var/local/kmc/bin/su ]")}
    if kpm:
        data["kpm_version"] = re.sub(r"\x1b\[[0-9;]*m", "", g.run(f"{KPM} version 2>&1")[1]).strip()
    ctx.state["kpm"] = kpm
    if not kpm:
        return FAIL, "no KPM at /var/local/kmc/bin/kpm: users can't install the package", data
    return PASS, data.get("kpm_version", "").splitlines()[0] if data.get("kpm_version") else "KPM present", data


@check("commands", "Commands the package and binary run are present")
def commands(ctx):
    script = "; ".join(f"command -v {shlex.quote(c)} >/dev/null 2>&1 && echo 'y {c}' || echo 'n {c}'"
                       for c in COMMANDS + DEV_COMMANDS)
    have = {}
    for line in ctx.guest.out(script).splitlines():
        flag, name = line.split(" ", 1)
        have[name] = flag == "y"
    missing = [c for c in COMMANDS if not have.get(c)]
    dev_missing = [c for c in DEV_COMMANDS if not have.get(c)]
    ctx.state["curl"] = have.get("curl")
    data = {"missing": missing, "dev_missing": dev_missing}
    if missing:
        return FAIL, f"missing: {', '.join(missing)}", data
    if dev_missing:
        return WARN, f"all present; dev/harness tools missing: {', '.join(dev_missing)}", data
    return PASS, f"all {len(COMMANDS)} present (and the dev tools)", data


@check("sqlite3", "sqlite3 supports -readonly and -json (src/catalog.rs)")
def sqlite3(ctx):
    rc, out, err = ctx.guest.run("sqlite3 -readonly -json :memory: 'SELECT 1 AS a'")
    version = ctx.guest.run("sqlite3 --version")[1].strip()
    data = {"version": version, "json_output": out.strip()}
    if rc != 0 or json.loads(out or "null") != [{"a": 1}]:
        return FAIL, f"sqlite3 {version.split(' ')[0]}: -json/-readonly failed: {err.strip() or out.strip()}", data
    return PASS, f"sqlite3 {version.split(' ')[0]}", data


@check("lipc", "lipc properties the binary and scripts read")
def lipc(ctx):
    g = ctx.guest
    props = {
        "wifid cmState": ("com.lab126.wifid", "cmState", True),
        "appmgrd activeApp": ("com.lab126.appmgrd", "activeApp", True),
        "amazonRegistrationService GetDeviceName": ("com.lab126.amazonRegistrationService", "GetDeviceName", False),
        "powerd preventScreenSaver (dev)": ("com.lab126.powerd", "preventScreenSaver", False),
    }
    data, bad, soft = {}, [], []
    for label, (service, prop, required) in props.items():
        rc, out, err = g.run(f"lipc-get-prop {service} {prop}", timeout=20)
        data[label] = out.strip() if rc == 0 else f"error: {err.strip() or rc}"
        if rc != 0:
            (bad if required else soft).append(label)
    if bad:
        return FAIL, f"can't read {', '.join(bad)}", data
    if soft:
        return WARN, f"can't read {', '.join(soft)} (optional)", data
    return PASS, f"Wi-Fi {data['wifid cmState']}; device name {data['amazonRegistrationService GetDeviceName']!r}", data


@check("paths", "Directories the package uses exist and are writable")
def paths(ctx):
    g = ctx.guest
    tests = {
        DOCS: f"[ -d {DOCS} ] && touch {DOCS}/.kws-w && rm {DOCS}/.kws-w",
        "/mnt/us/system/thumbnails": "[ -d /mnt/us/system/thumbnails ] && touch /mnt/us/system/thumbnails/.kws-w "
                                     "&& rm /mnt/us/system/thumbnails/.kws-w",
        "/var/local": "touch /var/local/.kws-w && rm /var/local/.kws-w",
        "/var/log": "touch /var/log/.kws-w && rm /var/log/.kws-w",
        "/etc/upstart": "[ -d /etc/upstart ]",
        "/var/local/mesquite": "[ -d /var/local/mesquite ]",
    }
    data = {p: g.ok(t) for p, t in tests.items()}
    bad = [p for p, ok in data.items() if not ok]
    if bad:
        return FAIL, f"not usable: {', '.join(bad)}", data
    return PASS, "all present and writable", data


@check("catalog_schema", "The catalog (cc.db) has the tables and columns the binary reads")
def catalog_schema(ctx):
    g = ctx.guest
    if not g.ok(f"[ -f {CC} ]"):
        return FAIL, f"no {CC}", None
    data, missing = {}, []
    for table, cols in (("Entries", ENTRIES_COLUMNS), ("Series", SERIES_COLUMNS)):
        have = set(g.sql(CC, f"SELECT name FROM pragma_table_info('{table}')").split())
        data[table] = len(have)
        missing += [f"{table}.{c}" for c in cols if c not in have]
    if missing:
        return FAIL, f"missing: {', '.join(missing)}", data
    return PASS, f"Entries and Series have all {len(ENTRIES_COLUMNS) + len(SERIES_COLUMNS)} columns used", data


@check("appreg_schema", "The app registry (appreg.db) has the tables register-app.sh writes")
def appreg_schema(ctx):
    g = ctx.guest
    missing = []
    for table, cols in APPREG_COLUMNS.items():
        have = set(g.sql(APPREG, f"SELECT name FROM pragma_table_info('{table}')").split())
        missing += [f"{table}.{c}" for c in cols if c not in have]
    # How a stock app is registered: our rows copy its shape.
    sample = g.sql(APPREG, "SELECT value FROM properties WHERE name = 'command' AND value LIKE '%mesquite%' LIMIT 1")
    data = {"stock_mesquite_command": sample}
    if missing:
        return FAIL, f"missing: {', '.join(missing)}", data
    if not sample:
        return WARN, "tables are there, but no stock app is registered as a mesquite command", data
    return PASS, f"tables present; stock apps use {sample!r}", data


@check("upstart", "upstart events the jobs use (framework_ready, stopping filesystems)")
def upstart(ctx):
    g = ctx.guest
    version = g.run("/sbin/initctl --version | head -1")[1].strip()
    emits_ready = g.run("grep -ls 'framework_ready' /etc/upstart/*.conf | head -5")[1].split()
    filesystems = g.ok("[ -f /etc/upstart/filesystems.conf ]")
    data = {"version": version, "framework_ready_mentioned_in": emits_ready, "filesystems_job": filesystems}
    problems = []
    if not emits_ready:
        problems.append("nothing in /etc/upstart mentions framework_ready (the autostart job's trigger)")
    if not filesystems:
        problems.append("no filesystems job (the service stops on 'stopping filesystems')")
    if problems:
        return FAIL, "; ".join(problems), data
    return PASS, f"{version}; framework_ready used by {len(emits_ready)} jobs", data


@check("catalog_endpoint", "The catalog's ChangeRequest endpoint answers (127.0.0.1:9101, ADR 0016)")
def catalog_endpoint(ctx):
    g = ctx.guest
    token = g.run("wc -c < /tmp/session_token")[1].strip()
    if not token:
        return FAIL, "no /tmp/session_token", None
    reply = change_request(ctx, [])
    unauth = g.run("curl -s -m 10 -o /dev/null -w '%{http_code}' -X POST -d '{}' "
                   "http://127.0.0.1:9101/change")[1].strip()
    data = {"token_bytes": int(token), "empty_request": reply, "without_token": unauth}
    if reply.get("ok") is not True:
        return FAIL, f"an empty ChangeRequest wasn't accepted: {reply}", data
    ctx.state["catalog_endpoint"] = True
    return PASS, f"accepts a ChangeRequest with the token; {unauth} without it", data


# ---- installing with KPM ----

@check("kpm_install", "The package installs with the device's KPM")
def kpm_install(ctx):
    g = ctx.guest
    if not g.ok(f"[ -x {KPM} ]"):
        raise CheckFailed(SKIP, "no KPM on the device")
    data = {}
    if g.ok(f"[ -d /mnt/us/kmc/kpm/packages/{PKG} ] || [ -x {BIN} ]"):
        log("the package is already installed; uninstalling it first")
        rc, out, _ = g.run(f"{KPM} -y uninstall {PKG} 2>&1", timeout=180)
        data["preexisting_uninstall"] = out.strip()[-2000:]
    g.put(ctx.kpkg.read_bytes(), "/tmp/kws-compat.kpkg")
    started = time.monotonic()
    rc, out, err = g.run(f"{KPM} -y install file:///tmp/kws-compat.kpkg 2>&1; rc=$?; rm -f /tmp/kws-compat.kpkg; "
                         "exit $rc", timeout=300)
    data["kpm_output"] = re.sub(r"\x1b\[[0-9;]*m", "", out).strip()[-3000:]
    data["seconds"] = round(time.monotonic() - started, 1)
    if rc != 0:
        return FAIL, f"kpm install exited {rc}", data
    rc, out, err = g.run(f"{BIN} status --state {DIR}/state", timeout=60)
    data["binary_status"] = {"rc": rc, "output": (out + err).strip()[-1000:]}
    installed = {
        "binary": g.ok(f"[ -x {BIN} ]"),
        "binary_runs": rc == 0,
        "upstart_jobs": g.ok("[ -f /etc/upstart/kindle-wireless-sync.conf ] && "
                             "[ -f /etc/upstart/kindle-wireless-sync-autostart.conf ]"),
        "app_registered": g.sql(APPREG, f"SELECT count(*) FROM handlerIds WHERE handlerId = '{APP_ID}'") == "1",
        "scriptlet": g.ok(f"[ -x {DOCS}/WirelessSync.sh ]"),
        "ui_readable": g.ok(f"[ \"$(stat -c %a {DIR}/ui)\" = 755 ] && [ \"$(stat -c %a {DIR}/ui/index.html)\" = 644 ]"),
        "root_back_to_ro": g.ok("mount | grep ' / ' | grep -q '(ro'"),
    }
    data["installed"] = installed
    bad = [k for k, v in installed.items() if not v]
    ctx.state["installed"] = installed["binary"]
    if bad:
        return FAIL, f"installed, but: {', '.join(bad)}", data
    return PASS, f"installed in {data['seconds']}s", data


@check("service", "The service runs and its API answers (ADR 0011)")
def service(ctx):
    g = ctx.guest
    ctx.needs("installed", "the package isn't installed")
    took = g.wait_for("/sbin/initctl status kindle-wireless-sync | grep -q running && "
                      f"curl -s -m 5 {API}/status | grep -q '\"ok\":true'", timeout=60)
    status = api_status(ctx)
    log_text = g.run("cat /var/log/kindle-wireless-sync.log 2>/dev/null | tail -40")[1]
    data = {"status": status, "log_tail": log_text.strip().splitlines()[-15:]}
    if took is None:
        return FAIL, "the service didn't start, or its API didn't answer within 60s", data
    problems = []
    if status.get("version") != ctx.version:
        problems.append(f"the API reports version {status.get('version')}, not {ctx.version}")
    if "lipc not available" in log_text:
        problems.append("the daemon found no lipc (it assumes Wi-Fi is always up)")
    if "cannot listen" in log_text:
        problems.append("the API couldn't listen")
    ctx.state["service"] = not problems
    if problems:
        return FAIL, "; ".join(problems), data
    return PASS, f"running; API answers (state {status.get('state')}, Wi-Fi {status.get('wifi')})", data


@check("autostart", "The autostart job starts the service (ADR 0010)")
def autostart(ctx):
    g = ctx.guest
    ctx.needs("installed", "the package isn't installed")
    g.run("/sbin/initctl stop kindle-wireless-sync", timeout=60)
    g.run(f"touch {DIR}/autostart && rm -f {DIR}/boot_attempts")
    g.run("/sbin/initctl start kindle-wireless-sync-autostart", timeout=60)
    took = g.wait_for("/sbin/initctl status kindle-wireless-sync | grep -q running", timeout=30)
    attempts = g.run(f"cat {DIR}/boot_attempts 2>/dev/null")[1].strip()
    g.run(f"rm -f {DIR}/autostart {DIR}/boot_attempts")
    data = {"boot_attempts_after_start": attempts}
    if took is None:
        g.run("/sbin/initctl start kindle-wireless-sync", timeout=60)
        return FAIL, "the autostart job ran, but the service didn't start", data
    return PASS, f"the service started (boot_attempts {attempts or 'unset'})", data


@check("device_name", "The Kindle's name reaches the binary (ADR 0015)", required=False)
def device_name(ctx):
    name = ctx.guest.run("lipc-get-prop com.lab126.amazonRegistrationService GetDeviceName")[1].strip()
    if not name:
        return WARN, "GetDeviceName is empty: Calibre will see the default name", None
    return PASS, repr(name), {"name": name}


# ---- the library ----

@check("scanner_indexing", "A book copied into documents/ is indexed, and the catalog announces it")
def scanner_indexing(ctx):
    g = ctx.guest
    g.run("rm -f /tmp/kws-ccat.log")
    ctx.state["scanner_errors_before"] = scanner_errors(ctx)
    # Watch for the event the binary waits on (src/kindle.rs, watch_catalog).
    g.run("(timeout 150 lipc-wait-event -m com.lab126.ccat updated > /tmp/kws-ccat.log 2>&1 &) ; sleep 1")
    g.put(b"Kindle Wireless Sync compatibility check: a book in documents/.\n", TOP_BOOK)
    g.run(f"mkdir -p {SUB_DIR}")
    g.put(b"Kindle Wireless Sync compatibility check: a book in a subfolder.\n", SUB_BOOK)
    start = time.monotonic()
    found = None
    while time.monotonic() - start < 120:
        found = entry(ctx, TOP_BOOK)
        if found:
            break
        time.sleep(2)
    took = round(time.monotonic() - start, 1)
    time.sleep(2)
    events = g.run("cat /tmp/kws-ccat.log")[1].split()
    data = {"seconds": took, "entry": found, "ccat_updated_events": events.count("updated")}
    if not found:
        return FAIL, "the book wasn't in cc.db after 120s", data
    ctx.state["top_entry"] = found
    if not events:
        return FAIL, f"indexed in {took}s, but com.lab126.ccat sent no 'updated' event", data
    return PASS, f"indexed in {took}s as {found[2]}; ccat 'updated' seen", data


@check("subfolder_indexing", "Books in subfolders of documents/ are indexed (ADR 0023)", required=False)
def subfolder_indexing(ctx):
    ctx.needs("top_entry", "the top-level book wasn't indexed")
    fields = "p_uuid, p_cdeKey, p_cdeType, p_visibilityState, p_isVisibleInHome, p_isArchived"
    start = time.monotonic()
    sub = None
    while time.monotonic() - start < 90:
        sub = entry(ctx, SUB_BOOK, fields)
        if sub:
            break
        time.sleep(2)
    top = entry(ctx, TOP_BOOK, fields)
    before, after = ctx.state.get("scanner_errors_before"), scanner_errors(ctx)
    data = {"subfolder": sub, "top_level": top, "scanner_errors_before": before, "scanner_errors_after": after}
    if not sub:
        refused = f"; the catalog refused {after - before} of the scanner's posts meanwhile (/var/log/messages)" \
            if before is not None and after is not None and after > before else ""
        return FAIL, f"the book in {SUB_DIR} wasn't indexed within 90s{refused}", data
    ctx.state["sub_entry"] = sub
    if top and sub[3:] != top[3:]:
        return WARN, f"indexed, but its flags differ from a top-level book's ({sub[3:]} vs {top[3:]})", data
    return PASS, "indexed, with the same flags as a top-level book", data


@check("series", "Native series through ChangeRequest, and the binary removes them (ADR 0016)")
def series(ctx):
    ctx.needs("catalog_endpoint", "the ChangeRequest endpoint doesn't work")
    ctx.needs("sub_entry", "needs two indexed test books")
    g = ctx.guest
    top, sub = ctx.state["top_entry"], ctx.state["sub_entry"]
    # Shapes from src/series.rs (plan): rows, container, then the members.
    key = "KWCOMPATST"
    sid = f"urn:collection:1:asin-{key}"
    container = str(uuid.uuid4())
    titles = [{"display": "KWS compat", "collation": "KWS compat", "pronunciation": "KWS compat", "language": "en"}]
    credits = [{"kind": "Author", "name": {"display": "KWS", "collation": "KWS", "pronunciation": "KWS",
                                           "language": "en"}}]
    commands = []
    for pos, e in enumerate((top, sub), start=1):
        commands.append({"insertOr": {"table": "SERIES", "onConflict": "REPLACE", "entry": {
            "seriesId": sid, "itemCdeKey": e[1], "itemType": "Entry:Item", "itemPosition": pos,
            "itemPositionLabel": str(pos), "seriesOrderType": "ordered"}}})
    commands.append({"insertOr": {"onConflict": "IGNORE", "entry": {
        "type": "Entry:Item:Series", "uuid": container, "cdeKey": key, "cdeType": "series", "cdeGroup": sid,
        "titles": titles, "credits": credits, "displayObjects": [{"ref": "titles"}, {"ref": "credits"}],
        "mimeType": "application/x-kindle-series", "thumbnail": "", "isArchived": 1, "isVisibleInHome": 1,
        "isLatestItem": 1, "originType": -1, "virtualCollectionCount": 2}}})
    commands.append({"update": {"uuid": container, "type": "Entry:Item:Series", "cdeType": "series",
                                "titles": titles, "credits": credits, "thumbnail": "", "virtualCollectionCount": 2,
                                "isProcessed": 1, "members": [top[0], sub[0]]}})
    for e in (top, sub):
        commands.append({"update": {"uuid": e[0], "seriesState": 0, "originType": 0}})
    reply = change_request(ctx, commands)
    if reply.get("ok") is not True:
        return FAIL, f"the catalog refused the series: {reply}", {"reply": reply}
    row = g.sql(CC, "SELECT p_memberCount, p_isProcessed, p_titles_0_nominal FROM Entries "
                    f"WHERE p_cdeKey = '{key}' AND p_cdeType = 'series'")
    rows = g.sql(CC, f"SELECT count(*) FROM Series WHERE d_seriesId = '{sid}'")
    data = {"reply": reply, "container": row, "series_rows": rows}
    made = row.startswith("2|1|") and rows == "2"
    # The binary's own removal (uninstall.sh runs it): reads the catalog
    # with src/series.rs's queries and dissolves our series.
    if ctx.state.get("installed"):
        rc, out, err = g.run(f"{BIN} series --books {DOCS} --catalog {CC} --remove-all", timeout=120)
        data["remove_all"] = {"rc": rc, "output": (out + err).strip()}
        removed = rc == 0 and g.sql(CC, f"SELECT count(*) FROM Entries WHERE p_cdeKey = '{key}'") == "0" \
            and g.sql(CC, f"SELECT count(*) FROM Series WHERE d_seriesId = '{sid}'") == "0"
    else:
        removed = None
    if not removed:
        # Don't leave the test series behind.
        change_request(ctx, [{"update": {"uuid": e[0], "seriesState": 1, "originType": "null"}} for e in (top, sub)]
                       + [{"delete": {"table": "SERIES", "seriesId": sid, "itemCdeKey": e[1]}} for e in (top, sub)]
                       + [{"update": {"uuid": container, "type": "Entry:Item:Series", "cdeType": "series",
                                      "members": []}}, {"delete": {"uuid": container}}])
    if not made:
        return FAIL, f"the series wasn't built as expected (container {row!r}, {rows} Series rows)", data
    if removed is False:
        return FAIL, f"the series was made, but `series --remove-all` didn't remove it: {data['remove_all']}", data
    return PASS, "made a two-book series" + (" and the binary removed it" if removed else ""), data


@check("catalog_delete", "A deleted book leaves the catalog at once through ChangeRequest (ADR 0005)")
def catalog_delete(ctx):
    ctx.needs("top_entry", "no indexed test book")
    g = ctx.guest
    uuids = [e[0] for e in (ctx.state.get("top_entry"), ctx.state.get("sub_entry")) if e]
    g.run(f"rm -rf {TOP_BOOK} {SUB_DIR} {TOP_BOOK[:-4]}.sdr")
    reply = change_request(ctx, [{"delete": {"uuid": u}} for u in uuids])
    gone = g.sql(CC, f"SELECT count(*) FROM Entries WHERE p_location LIKE '{DOCS}/kws-compat-{RUN}%'")
    # The scanner mustn't put them back once it notices the files are gone.
    time.sleep(30)
    later = g.sql(CC, f"SELECT count(*) FROM Entries WHERE p_location LIKE '{DOCS}/kws-compat-{RUN}%'")
    g.run(f"rm -rf {SUB_DIR} {TOP_BOOK[:-4]}.sdr")
    data = {"reply": reply, "left_at_once": gone, "left_after_30s": later}
    if reply.get("ok") is not True or gone != "0":
        return FAIL, f"the entries weren't removed: {reply}, {gone} left", data
    if later != "0":
        return FAIL, f"removed, but {later} came back within 30s", data
    return PASS, f"removed {len(uuids)} entries at once; not re-added", data


# ---- network ----

@check("wifi_events", "Wi-Fi state changes arrive over lipc (src/network.rs)", required=False)
def wifi_events(ctx):
    g = ctx.guest
    state = g.run("lipc-get-prop com.lab126.wifid cmState")[1].strip()
    if state != "CONNECTED":
        log("joining the emulator's Wi-Fi (Kindle-QEMU)")
        g.run("echo '{ essid = \"Kindle-QEMU\", secured = \"no\" }' | lipc-hash-prop com.lab126.wifid createProfile")
        g.run("lipc-set-prop com.lab126.wifid cmConnect Kindle-QEMU")
        if g.wait_for("[ \"$(lipc-get-prop com.lab126.wifid cmState)\" = CONNECTED ]", timeout=90) is None:
            state = g.run("lipc-get-prop com.lab126.wifid cmState")[1].strip()
            return WARN, (f"Wi-Fi stuck at {state}. On the emulator this is usually the host: a real Kindle on "
                          "USB, or Tailscale managing DNS (CLAUDE.md)"), {"cmState": state}
    g.run("rm -f /tmp/kws-wifi.log; (timeout 120 lipc-wait-event -m com.lab126.wifid cmStateChange "
          "> /tmp/kws-wifi.log 2>&1 &) ; sleep 1")
    g.run("lipc-set-prop com.lab126.wifid enable 0; sleep 3; lipc-set-prop com.lab126.wifid enable 1")
    back = g.wait_for("[ \"$(lipc-get-prop com.lab126.wifid cmState)\" = CONNECTED ]", timeout=90)
    time.sleep(2)
    events = re.findall(r'cmStateChange "([A-Z_]+)"', g.run("cat /tmp/kws-wifi.log")[1])
    data = {"events": events}
    if ctx.state.get("service"):
        st = api_status(ctx)
        data["service_wifi"] = st.get("wifi") if st else None
    if back is None:
        return WARN, f"Wi-Fi didn't reconnect after being toggled; events: {events}", data
    if "CONNECTED" not in events:
        return FAIL, f"no cmStateChange \"CONNECTED\" event while Wi-Fi reconnected (saw {events})", data
    if ctx.state.get("service") and data.get("service_wifi") is not True:
        return FAIL, f"events arrived ({events}), but the service reports Wi-Fi {data.get('service_wifi')}", data
    return PASS, f"events: {' → '.join(events)}" + ("; the service follows them" if ctx.state.get("service") else ""), data


# ---- the app ----

@check("app_launch", "The Library tile's scriptlet opens the app in mesquite (ADR 0004)")
def app_launch(ctx):
    ctx.needs("installed", "the package isn't installed")
    g = ctx.guest
    rc, out, err = g.run(f"sh {DOCS}/WirelessSync.sh", timeout=60)
    took = g.wait_for(f"[ \"$(lipc-get-prop com.lab126.appmgrd activeApp)\" = {APP_ID} ]", timeout=45)
    data = {"scriptlet_output": (out + err).strip()}
    if took is None:
        app = g.run("lipc-get-prop com.lab126.appmgrd activeApp")[1].strip()
        return FAIL, f"the app didn't come to the front (activeApp {app})", data
    time.sleep(6)  # let it load /status and draw
    data["screenshot"] = ctx.screenshot("app-launch.png")
    g.run(f"lipc-set-prop com.lab126.appmgrd stop app://{APP_ID}")
    g.wait_for("[ \"$(lipc-get-prop com.lab126.appmgrd activeApp)\" = com.lab126.KPPMainApp ]", timeout=30)
    return PASS, f"in front after {took:.0f}s", data


@check("ui_pages_fit", "Every page of the app fits on one screen (ADR 0020)")
def ui_fit(ctx):
    results, env, count = fitcheck.run(ctx.guest, ctx.repo, ctx.work, ctx.screenshot)
    checks = fitcheck.evaluate(results, env, count)
    # The first is this check's own result; the rest are added after it.
    first, rest = checks[0], checks[1:]
    ctx.extra = [result(cid, title, status, detail, required, data) for cid, title, status, detail, required, data
                 in rest]
    return first[2], first[3], first[5]


# ---- removing ----

@check("kpm_uninstall", "The package uninstalls cleanly with KPM (ADR 0012)")
def kpm_uninstall(ctx):
    ctx.needs("installed", "the package isn't installed")
    g = ctx.guest
    rc, out, _ = g.run(f"{KPM} -y uninstall {PKG} 2>&1", timeout=300)
    left = {
        "dir": g.ok(f"[ -e {DIR} ]"),
        "upstart_jobs": g.ok("ls /etc/upstart/kindle-wireless-sync*.conf >/dev/null 2>&1"),
        "app_registered": g.sql(APPREG, f"SELECT count(*) FROM handlerIds WHERE handlerId = '{APP_ID}'") != "0",
        "scriptlet": g.ok(f"[ -e {DOCS}/WirelessSync.sh ]"),
        "service_running": g.ok("pgrep -f '[k]indle-wireless-sync daemon' >/dev/null"),
        "root_rw": not g.ok("mount | grep ' / ' | grep -q '(ro'"),
    }
    data = {"kpm_output": re.sub(r"\x1b\[[0-9;]*m", "", out).strip()[-2000:], "left_behind": left}
    if rc != 0:
        return FAIL, f"kpm uninstall exited {rc}", data
    bad = [k for k, v in left.items() if v]
    if bad:
        return FAIL, f"left behind: {', '.join(bad)}", data
    return PASS, "removed everything", data


def run_all(ctx, only=None, skip=()):
    out = []
    for check_id, title, required, fn in CHECKS:
        if (only and check_id not in only) or check_id in skip:
            continue
        log(f"check {check_id}: {title}")
        start = time.monotonic()
        ctx.extra = []
        try:
            status, detail, data = fn(ctx)
        except CheckFailed as e:
            status, detail, data = e.status, e.detail, e.data
        except Exception as e:  # noqa: BLE001 - one broken check shouldn't stop the run
            status, detail, data = ERROR, f"{type(e).__name__}: {e}", None
        r = result(check_id, title, status, detail, required, data, time.monotonic() - start)
        log(f"  {status.upper()}: {detail}")
        out.append(r)
        for x in ctx.extra:
            log(f"  {x['id']}: {x['status'].upper()}: {x['detail']}")
            out.append(x)
    return out
