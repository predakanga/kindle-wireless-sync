#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Static fingerprints of Kindle firmware: what the compat checks depend on,
read from each firmware's root filesystem without booting it. Versions with
the same fingerprint should give the same check results, so one compat run
covers them all (README, "Choosing what to run").

Reads eink-emulator's imported firmware (firmware/versions/MODEL/VERSION/
rootfs.img) with e2fsprogs' debugfs. Don't search rootfs.img directly:
files are split across blocks (research note, "Catalog support for series
across firmware").

  fingerprint.py MODEL VERSION [MODEL VERSION ...]   one JSON line each
  fingerprint.py --all                               every kindlehf version
  fingerprint.py --plan                              compat runs covering every version and model
  fingerprint.py --group FILE                        versions grouped by fingerprint

Firmware that isn't imported yet is downloaded, fingerprinted and deleted
again (--keep keeps it), so only one image is on disk at a time.
"""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from checks import APPREG_COLUMNS, COMMANDS, DEV_COMMANDS, ENTRIES_COLUMNS, SERIES_COLUMNS
from common import log

REPO = Path(__file__).resolve().parents[2]
KINDLEHF = (5, 16, 3)  # the first hard-float firmware (research note)
PATH_DIRS = ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
# The fields src/series.rs's ChangeRequest commands send, which ccat's
# change.lua must map to columns (its column_specs).
CHANGE_FIELDS = ["seriesState", "originType", "virtualCollectionCount", "isProcessed", "members", "cdeGroup",
                 "mimeType", "displayObjects", "titles", "credits", "thumbnail", "isArchived", "isVisibleInHome",
                 "isLatestItem", "type", "uuid", "cdeKey", "cdeType", "seriesId", "itemCdeKey", "itemType",
                 "itemPosition", "itemPositionLabel", "seriesOrderType"]
# SQLITE_SOURCE_ID (date and hash prefix) to release, for those seen.
SQLITE_RELEASES = {
    "2018-12-01 bf8c1b2b7a59": "3.26.0",  # compat run, Paperwhite 6 5.17.1.0.2
    "2024-10-21 03a9703e27c4": "3.47.0",  # libsqlite3's version string, 5.18.6
}


def vkey(v):
    return tuple(int(x) for x in v.split("."))


def find_debugfs():
    for p in [shutil.which("debugfs"), "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
              "/usr/local/opt/e2fsprogs/sbin/debugfs"]:
        if p and Path(p).exists():
            return p
    sys.exit("debugfs not found: brew install e2fsprogs")


class Image:
    """Read-only access to an ext4 image through debugfs."""

    def __init__(self, path, debugfs, work):
        self.path, self.debugfs, self.work = str(path), debugfs, Path(work)

    def run(self, cmd):
        return subprocess.run([self.debugfs, "-R", cmd, self.path], capture_output=True, text=True,
                              errors="replace").stdout

    def listdir(self, path):
        """Names in a directory (`ls -p`: /inode/mode/uid/gid/name/size/)."""
        names = []
        for line in self.run(f"ls -p {path}").splitlines():
            parts = line.split("/")
            if len(parts) >= 7 and parts[5] not in (".", ".."):
                names.append(parts[5])
        return names

    def exists(self, path):
        return "Inode:" in self.run(f"stat {path}")

    def resolve(self, path, depth=0):
        """Follows symlinks, as debugfs's dump doesn't."""
        st = self.run(f"stat {path}")
        m = re.search(r'Fast link dest: "([^"]+)"', st)
        if not m or depth > 8:
            return path
        dest = m.group(1)
        if not dest.startswith("/"):
            dest = str(Path(path).parent / dest)
        return self.resolve(dest, depth + 1)

    def read(self, path):
        out = self.work / "dump"
        out.unlink(missing_ok=True)
        self.run(f"dump {self.resolve(path)} {out}")
        return out.read_bytes() if out.exists() else None

    def read_dir(self, path):
        out = self.work / "rdump"
        shutil.rmtree(out, ignore_errors=True)
        out.mkdir()
        self.run(f"rdump {path} {out}")
        sub = out / Path(path).name
        return sub if sub.exists() else None


def sha(data):
    return hashlib.sha256(data).hexdigest()[:16] if data is not None else None


def table_columns(sqls, name):
    """A table's final columns: its newest Create block, plus any column an
    Update adds (5.20.1's Create block lags its updates)."""
    blocks = re.findall(r"--## Create: " + name + r":(\d+)\n(.*?)(?=\n--## |\Z)", sqls, re.S)
    if not blocks:
        return None, set()
    n, body = max(blocks, key=lambda b: int(b[0]))
    cols = set()
    for line in body[body.index("(") + 1:].splitlines():
        if line.startswith(")"):
            break
        tok = line.split("--")[0].strip().rstrip(",").strip()
        if tok and tok.split()[0].upper() not in ("UNIQUE", "PRIMARY", "CONSTRAINT"):
            cols.add(tok.split()[0])
    cols |= set(re.findall(r"--## Update: " + name + r":\d+:\d+\nALTER TABLE " + name + r" ADD COLUMN (\w+)", sqls))
    return int(n), cols


# Recorded, but not part of the grouping: no check's outcome depends on them
# (sqlite3 only needs -readonly since ADR 0025; catalog_missing covers the
# columns).
INFO_KEYS = ("sqlite3_source_id", "sqlite3_version", "sqlite3_json", "entries_version", "series_version")


def fingerprint(img):
    """`features` decide the grouping; `info` and `files` (hashes) are only
    for spotting where behaviour might still differ."""
    f, files = {}, {}
    pretty = (img.read("/etc/prettyversion.txt") or b"").decode(errors="replace").strip()
    f["prettyversion_parses"] = bool(re.match(r"Kindle 5(\.\d+)+\b", pretty))
    f["kindlehf"] = img.exists("/lib/ld-linux-armhf.so.3")

    on_path = set()
    for d in PATH_DIRS:
        on_path |= set(img.listdir(d))
    f["commands_missing"] = sorted(c for c in COMMANDS if not (img.exists(c) if c.startswith("/") else c in on_path))
    f["dev_commands_missing"] = sorted(c for c in DEV_COMMANDS
                                       if not (img.exists(c) if c.startswith("/") else c in on_path))

    # The CLI has its own SQLite, which can differ from libsqlite3's: its
    # SQLITE_SOURCE_ID ("DATE TIME HASH") identifies the release.
    sqlite_bin = img.read("/usr/bin/sqlite3")
    found = re.search(rb"(20\d\d-\d\d-\d\d) \d\d:\d\d:\d\d ([0-9a-f]{12})", sqlite_bin or b"")
    source = f"{found.group(1).decode()} {found.group(2).decode()}" if found else None
    f["sqlite3_source_id"] = source
    f["sqlite3_version"] = SQLITE_RELEASES.get(source)
    # -json arrived in 3.33.0 (2020-08-14); only recorded since ADR 0025.
    f["sqlite3_json"] = bool(source) and source[:10] >= "2020-08-14"
    files["sqlite3"] = sha(sqlite_bin)

    sqls_b = img.read("/usr/share/cc/cc.sqls")
    sqls = (sqls_b or b"").decode(errors="replace")
    en, ecols = table_columns(sqls, "Entries")
    sn, scols = table_columns(sqls, "Series")
    f["entries_version"], f["series_version"] = en, sn
    f["catalog_missing"] = [f"Entries.{c}" for c in ENTRIES_COLUMNS if c not in ecols] + \
                           [f"Series.{c}" for c in SERIES_COLUMNS if c not in scols]
    files["cc.sqls"] = sha(sqls_b)

    ccat = img.read_dir("/usr/lib/ccat")
    change = (ccat / "change.lua").read_text(errors="replace") if ccat and (ccat / "change.lua").exists() else ""
    specs = re.search(r"local column_specs\s*=\s*\{(.*?)\n\}", change, re.S)
    keys = set(re.findall(r"^\s*(\w+)\s*=", specs.group(1), re.M)) if specs else set()
    f["change_fields_missing"] = [k for k in CHANGE_FIELDS if k not in keys]
    f["change_series_ops"] = all(s in change for s in ("insertOr_series", "delete_series",
                                                        "INSERT OR REPLACE INTO Series"))
    files["ccat"] = sha(b"".join(p.read_bytes() for p in sorted(ccat.rglob("*")) if p.is_file())) if ccat else None

    # libappreg creates appreg.db's tables; its CREATE TABLE statements are
    # in the library as text.
    lib = b""
    for name in img.listdir("/usr/lib"):
        if name.startswith("libappreg.so"):
            lib = img.read(f"/usr/lib/{name}") or b""
            if b"CREATE TABLE" in lib:
                break
    missing = []
    for t, cols in APPREG_COLUMNS.items():
        m = re.search(rb"CREATE TABLE " + t.encode() + rb" \((.*?)\);", lib, re.S)
        have = set(re.findall(rb"[(,]\s*(\w+)", b"," + m.group(1))) if m else set()
        missing += [f"{t}.{c}" for c in cols if c.encode() not in have]
    f["appreg_missing"] = missing
    files["libappreg"] = sha(lib or None)

    upstart = img.read_dir("/etc/upstart")
    confs = sorted(upstart.glob("*.conf")) if upstart else []
    f["upstart_filesystems_job"] = any(p.name == "filesystems.conf" for p in confs)
    f["upstart_framework_ready"] = any(b"framework_ready" in p.read_bytes() for p in confs)
    files["upstart"] = sha(b"".join(p.name.encode() + p.read_bytes() for p in confs))

    webkit = sorted(n for n in img.listdir("/usr/lib") if re.match(r"libwebkit.*\.so\.\d+\.\d+", n))
    f["webkit"] = webkit
    files["webkit"] = sha(img.read(f"/usr/lib/{webkit[0]}")) if webkit else None
    info = {k: f.pop(k) for k in INFO_KEYS}
    return pretty, f, info, files


# ---- eink-emulator ----

def eink_versions(eink, model):
    out = subprocess.run([str(eink / "eink"), "versions", "--model", model], capture_output=True, text=True,
                         cwd=eink).stdout
    vs = [l.split()[0] for l in out.splitlines()[2:] if l.strip()]
    return sorted({v for v in vs if re.fullmatch(r"5(\.\d+)+", v) and vkey(v) >= KINDLEHF}, key=vkey)


def kindlehf_releases(eink):
    models = json.loads((eink / "models.json").read_text())
    names = models.keys() if isinstance(models, dict) else [m["id"] for m in models]
    avail = {}
    for m in sorted(n for n in names if n.startswith("kindle-")):
        vs = eink_versions(eink, m)
        if vs:
            avail[m] = vs
    return avail


def imported(eink, model, version):
    return (eink / "firmware" / "versions" / model / version / "rootfs.img").exists()


def plan(avail):
    """Fewest (model, version) runs covering every version and every model:
    a minimum edge cover (versions matched to models, newest first)."""
    match = {}

    def try_model(m, seen):
        for v in reversed(avail[m]):
            if v in seen:
                continue
            seen.add(v)
            if v not in match or try_model(match[v], seen):
                match[v] = m
                return True
        return False

    for m in sorted(avail, key=lambda m: len(avail[m])):
        try_model(m, set())
    runs = {(m, v) for v, m in match.items()}
    for v in sorted({v for vs in avail.values() for v in vs}, key=vkey):
        if v not in match:
            runs.add((min((m for m in avail if v in avail[m]), key=lambda m: len(avail[m])), v))
    return sorted(runs, key=lambda r: vkey(r[1]))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pairs", nargs="*", help="MODEL VERSION pairs")
    ap.add_argument("--all", action="store_true", help="every kindlehf version, preferring imported firmware")
    ap.add_argument("--plan", action="store_true", help="print the compat runs covering every version and model")
    ap.add_argument("--group", metavar="FILE", help="group the JSON lines in FILE by fingerprint")
    ap.add_argument("--skip-done", metavar="FILE", help="with --all: skip versions already in FILE")
    ap.add_argument("--keep", action="store_true", help="keep firmware this run downloaded")
    ap.add_argument("--eink-dir", default=str(REPO.parent / "eink-emulator"))
    args = ap.parse_args()
    eink = Path(args.eink_dir)

    if args.group:
        groups = {}
        for line in Path(args.group).read_text().splitlines():
            r = json.loads(line)
            groups.setdefault(json.dumps(r["features"], sort_keys=True), []).append(r)
        for i, (feat, rs) in enumerate(sorted(groups.items(), key=lambda g: vkey(g[1][0]["version"])), 1):
            rs.sort(key=lambda r: vkey(r["version"]))
            print(f"group {i}: {', '.join(r['version'] for r in rs)}")
            print(f"  {feat}")
        return

    if args.plan:
        for m, v in plan(kindlehf_releases(eink)):
            print(m, v)
        return

    if args.all:
        avail = kindlehf_releases(eink)
        done = set()
        if args.skip_done and Path(args.skip_done).exists():
            done = {json.loads(l)["version"] for l in Path(args.skip_done).read_text().splitlines() if l.strip()}
        planned = dict((v, m) for m, v in plan(avail))
        pairs = []
        for v in sorted({v for vs in avail.values() for v in vs}, key=vkey):
            if v in done:
                continue
            have = [m for m in avail if v in avail[m] and imported(eink, m, v)]
            pairs.append((have[0] if have else planned[v], v))
    else:
        if len(args.pairs) % 2:
            ap.error("give MODEL VERSION pairs")
        pairs = list(zip(args.pairs[::2], args.pairs[1::2]))

    debugfs = find_debugfs()
    for model, version in pairs:
        fw = eink / "firmware" / "versions" / model / version
        downloaded = False
        if not (fw / "rootfs.img").exists():
            log(f"downloading {model} {version}")
            r = subprocess.run([str(eink / "eink"), "download", "--model", model, version], cwd=eink,
                               stdout=sys.stderr, stderr=sys.stderr)
            if r.returncode != 0 or not (fw / "rootfs.img").exists():
                print(json.dumps({"model": model, "version": version, "error": "download failed"}), flush=True)
                continue
            downloaded = True
        try:
            with tempfile.TemporaryDirectory() as work:
                log(f"fingerprinting {model} {version}")
                pretty, features, info, files = fingerprint(Image(fw / "rootfs.img", debugfs, work))
            print(json.dumps({"model": model, "version": version, "prettyversion": pretty,
                              "features": features, "info": info, "files": files}), flush=True)
        finally:
            if downloaded and not args.keep:
                shutil.rmtree(fw)
                if not any(fw.parent.iterdir()):
                    fw.parent.rmdir()


if __name__ == "__main__":
    main()
