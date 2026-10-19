# SPDX-License-Identifier: GPL-3.0-or-later
"""The page-fit check: runs the real app (package/ui) on the Kindle with a
fake service (fitcheck/harness.js), one page per scenario, and collects
each page's measurements and a screenshot."""

import json
import shutil
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import png
from common import FAIL, PASS, WARN, log

APP_ID = "com.kindlewirelesssync.fitcheck"
APP_DIR = "/var/local/kws-fitcheck"
GUEST_PORT = 18326  # COLLECTOR in harness.js
HERE = Path(__file__).resolve().parent

# The same rows package/register-app.sh writes, for the test app.
REGISTER_SQL = f"""
INSERT OR IGNORE INTO interfaces (interface) VALUES ('application');
INSERT OR IGNORE INTO handlerIds (handlerId) VALUES ('{APP_ID}');
INSERT OR IGNORE INTO associations (handlerId, interface, contentId, defaultAssoc)
    VALUES ('{APP_ID}', 'application', 'GL:{APP_ID}', 0);
INSERT OR REPLACE INTO properties (handlerId, name, value) VALUES ('{APP_ID}', 'lipcId', '{APP_ID}');
INSERT OR REPLACE INTO properties (handlerId, name, value)
    VALUES ('{APP_ID}', 'command', '/usr/bin/mesquite -l {APP_ID} -c file://{APP_DIR}/');
INSERT OR REPLACE INTO properties (handlerId, name, value) VALUES ('{APP_ID}', 'supportedOrientation', 'U');
"""
UNREGISTER_SQL = f"""
DELETE FROM properties WHERE handlerId = '{APP_ID}';
DELETE FROM associations WHERE handlerId = '{APP_ID}';
DELETE FROM handlerIds WHERE handlerId = '{APP_ID}';
"""


def build_app(repo, out):
    """The real app's files, with harness.js loaded before script.js and
    its own app id."""
    ui = Path(repo) / "package" / "ui"
    out.mkdir(parents=True, exist_ok=True)
    for f in ("style.css", "script.js"):
        shutil.copy(ui / f, out / f)
    shutil.copy(HERE / "fitcheck" / "harness.js", out / "harness.js")
    index = (ui / "index.html").read_text()
    tag = '<script src="script.js"></script>'
    if tag not in index:
        raise RuntimeError(f"package/ui/index.html no longer loads script.js as {tag}")
    (out / "index.html").write_text(index.replace(tag, '<script src="harness.js"></script>\n    ' + tag))
    config = (ui / "config.xml").read_text()
    config = config.replace('id="com.kindlewirelesssync.app"', f'id="{APP_ID}"')
    (out / "config.xml").write_text(config.replace(">Wireless Sync<", ">KWS fit check<"))


class Collector:
    """The host end of the harness: hands out scenario numbers, and takes a
    screenshot of each page before letting the app move on."""

    def __init__(self, screenshot):
        self.screenshot = screenshot
        self.index = 0
        self.results = []
        self.env = None
        self.count = None
        self.done = threading.Event()
        self.progress = threading.Event()
        self.lock = threading.Lock()
        collector = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, code, body):
                data = json.dumps(body).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                if self.path == "/state":
                    with collector.lock:
                        self.reply(200, {"index": collector.index})
                else:
                    self.reply(404, {})

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                try:
                    body = json.loads(self.rfile.read(n) or b"{}")
                except ValueError:
                    body = {}
                if self.path == "/env":
                    collector.env = body
                    collector.count = len(body.get("scenarios") or []) or None
                    log(f"fit check: {collector.count} pages to check; "
                        f"the app gets {body.get('inner_width')}×{body.get('inner_height')} px")
                elif self.path == "/result":
                    collector.record(body)
                elif self.path == "/done":
                    collector.count = body.get("count", collector.count)
                    collector.done.set()
                collector.progress.set()
                self.reply(200, {})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def record(self, body):
        with self.lock:
            if body.get("index") != self.index:
                return  # a late reply for a scenario we already gave up on
            # Let the e-ink refresh finish before the screenshot.
            time.sleep(1.5)
            shot = self.screenshot(f"fit-{self.index:02d}-{body.get('name')}.png")
            if shot:
                body["screenshot"] = shot
                m = body.get("metrics")
                if body.get("keyboard") and m:
                    try:
                        m["keyboard_top"] = keyboard_top(shot, m["inner_width"], m["inner_height"])
                    except (OSError, ValueError) as e:
                        log(f"fit check: can't read {shot}: {e}")
            self.results.append(body)
            log(f"fit check: {describe(body)}")
            self.index += 1

    def skip(self, name, why):
        with self.lock:
            self.results.append({"index": self.index, "name": name, "error": why})
            self.index += 1

    def close(self):
        self.server.shutdown()


def keyboard_top(shot, inner_width, inner_height):
    """Where the on-screen keyboard starts, in the app's CSS pixels, from a
    screenshot: its top edge is the first row dark across the whole width
    (the app's own rules stop short at the body padding). None if no
    keyboard shows."""
    width, height, rows = png.read_grey(shot)
    scale = width / inner_width
    app_top = height - inner_height * scale  # the app runs to the bottom of the screen
    for y in range(int(app_top), height):
        row = rows[y]
        if sum(1 for v in row if v < 80) >= 0.99 * width:
            return round((y - app_top) / scale)
    return None


def fits(r):
    m = r.get("metrics") or {}
    if r.get("keyboard"):
        # Typing: the field must show above the keyboard. Enter moves on or
        # closes the keyboard, which brings the buttons back.
        return (not r.get("error") and r.get("reached") and m.get("keyboard_top") is not None
                and m.get("focused_bottom") is not None and m["focused_bottom"] <= m["keyboard_top"])
    return (not r.get("error") and r.get("reached") and m.get("overflow", 1) <= 0
            and m.get("h_overflow", 1) <= 0 and m.get("dialog_overflow", 0) <= 0 and not m.get("too_wide"))


def describe(r):
    if r.get("error"):
        return f"{r.get('name')}: ERROR {r['error']}"
    m = r.get("metrics") or {}
    spare = -m.get("overflow", 0)
    word = "fits" if fits(r) else "DOESN'T FIT"
    extra = ""
    if r.get("keyboard"):
        if m.get("keyboard_top") is None:
            return f"{r.get('name')}: no keyboard showed, so it can't be measured"
        spare = m["keyboard_top"] - (m.get("focused_bottom") or 0)
        extra = f" above the keyboard, which starts at {m['keyboard_top']}px"
    if m.get("dialog_overflow") is not None:
        extra += f", dialog {-m['dialog_overflow']}px clear of the bottom"
    if m.get("too_wide"):
        extra += f", too wide: {m['too_wide']}"
    return f"{r.get('name')}: {word} ({spare}px spare{extra})"


def run(guest, repo, work_dir, screenshot, scenario_timeout=60):
    """Runs every scenario. Returns (results, env)."""
    app = work_dir / "fitcheck-app"
    build_app(repo, app)
    guest.run(f"rm -rf {APP_DIR}", check=True)
    guest.put_tree(app, APP_DIR)
    # mesquite runs as the framework user: the app must be readable (install.sh).
    guest.run(f"chmod 755 {APP_DIR} && chmod 644 {APP_DIR}/*", check=True)
    guest.sql_write("/var/local/appreg.db", REGISTER_SQL)
    guest.run(f"rm -rf /var/local/mesquite/{APP_ID}")

    collector = Collector(screenshot)
    tunnel = guest.reverse_tunnel(GUEST_PORT, collector.port)
    start_app = f"lipc-set-prop com.lab126.appmgrd start app://{APP_ID}"
    stop_app = f"lipc-set-prop com.lab126.appmgrd stop app://{APP_ID}"
    try:
        guest.run(start_app, check=True)
        last_index, last_change = 0, time.monotonic()
        restarts = 0
        retried = set()
        while not collector.done.is_set():
            collector.progress.wait(5)
            collector.progress.clear()
            if collector.index != last_index:
                last_index, last_change = collector.index, time.monotonic()
                continue
            if time.monotonic() - last_change < scenario_timeout:
                continue
            # Stuck: the page never reported. Skip it and restart the app.
            restarts += 1
            if restarts > 5 or (collector.count and collector.index >= collector.count):
                log("fit check: the app stopped responding; giving up")
                break
            name = (collector.env or {}).get("scenarios", [None] * 99)[collector.index] \
                if collector.env else None
            # A busy emulator can lose a page: try it once more, then skip it.
            if collector.index in retried:
                log(f"fit check: no report for page {collector.index} ({name}) again; skipping it")
                collector.skip(name, f"no report within {scenario_timeout}s, twice")
            else:
                log(f"fit check: no report for page {collector.index} ({name}) in {scenario_timeout}s; "
                    "restarting the app to try again")
                retried.add(collector.index)
            guest.run(stop_app)
            time.sleep(3)
            guest.run(start_app)
            last_index, last_change = collector.index, time.monotonic()
    finally:
        guest.run(stop_app)
        tunnel.terminate()
        collector.close()
        try:
            guest.sql_write("/var/local/appreg.db", UNREGISTER_SQL)
        except RuntimeError as e:
            log(f"fit check: couldn't unregister the test app: {e}")
        guest.run(f"rm -rf {APP_DIR} /var/local/mesquite/{APP_ID}")
    return collector.results, collector.env, collector.count


def evaluate(results, env, count):
    """The check results for the page fit and the browser."""
    rows = [{k: r.get(k) for k in ("name", "reached", "keyboard", "dialog", "error", "screenshot")}
            | {"fits": fits(r), "metrics": r.get("metrics")} for r in results]
    plain = [r for r in results if not r.get("keyboard")]
    keyboard = [r for r in results if r.get("keyboard")]
    bad = [r["name"] for r in plain if not fits(r)]
    kb_bad = [r["name"] for r in keyboard if not fits(r)]
    missing = (count or 0) - len(results)
    checks = []
    if not results:
        status, detail = FAIL, "the fit-check app never reported"
    elif bad or missing > 0:
        status = FAIL
        detail = (f"{len(bad)} of {len(plain)} pages don't fit or couldn't be reached: {', '.join(bad)}"
                  if bad else "") + (f"; {missing} pages not checked" if missing > 0 else "")
    else:
        spare = min((-(r["metrics"]["overflow"]) for r in plain), default=None)
        status, detail = PASS, f"all {len(plain)} pages fit; the tightest has {spare}px spare"
    checks.append(("ui_pages_fit", "Every page of the app fits on one screen (ADR 0020)", status,
                   detail.strip("; "), True, {"pages": [r for r in rows if not r["keyboard"]]}))

    # Typing pages: the field being typed in must show above the keyboard.
    if keyboard:
        no_kb = [r["name"] for r in keyboard if not r.get("error")
                 and (r.get("metrics") or {}).get("keyboard_top") is None]
        covered = [n for n in kb_bad if n not in no_kb]
        if covered:
            kstatus, kdetail = FAIL, f"the keyboard covers the field on: {', '.join(covered)}"
        elif no_kb:
            # Focusing a field from script doesn't open the keyboard on
            # every firmware (5.16.2.1.1 on a Basic 2016 didn't).
            kstatus = WARN
            kdetail = f"the keyboard didn't open when a field got focus ({', '.join(no_kb)}), so it can't be measured"
        else:
            tops = sorted({r["metrics"]["keyboard_top"] for r in keyboard})
            kstatus, kdetail = PASS, f"fields stay above the keyboard (which starts at {tops[0]}px)"
        checks.append(("ui_pages_fit_keyboard", "Fields stay above the on-screen keyboard", kstatus,
                       kdetail, True, {"pages": [r for r in rows if r["keyboard"]]}))

    if env:
        has = env.get("has") or {}
        missing = [k for k in ("json", "query_selector_all", "add_event_listener") if not has.get(k)]
        checks.append(("ui_browser", "The app's browser (mesquite) has what the app needs",
                       FAIL if missing else PASS,
                       f"the browser lacks: {', '.join(missing)}" if missing else
                       f"{env.get('inner_width')}×{env.get('inner_height')} viewport; flexbox style "
                       f"{'accepted' if has.get('flex_style') else 'not accepted'}",
                       True, env))
    return checks
