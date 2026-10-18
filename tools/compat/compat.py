#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks whether a Kindle model and firmware version can run Kindle
Wireless Sync, on an eink-emulator instance (PLAN Phase 6).

Boots the emulated Kindle (creating and jailbreaking the instance on first
use), checks the prerequisites, runs every check in checks.py and prints
one JSON object on stdout, for appending to a JSONL file:

    tools/compat/compat.py kindle-paperwhite-6 5.20.1 >> compat.jsonl

Progress goes to stderr. Test runs boot with QEMU's -snapshot, so the
instance's disk is never changed by them. See tools/compat/README.md.
"""

import argparse
import datetime
import json
import platform
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import checks  # noqa: E402
from common import FAIL, PASS, WARN, log, result, summarise  # noqa: E402
from emulator import BootFailed, Emulator, free_port, port_free  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
SCHEMA = 1


# Short forms keep instance names, and so QEMU's socket paths, short.
ABBREVIATIONS = [("kindle-", ""), ("paperwhite-", "pw"), ("basic-", "kt"), ("scribe-", "ks"),
                 ("oasis-", "koa"), ("colorsoft", "cs"), ("voyage", "kv"), ("touch", "kt2")]
# macOS limits a Unix socket path to 103 bytes (sun_path).
MAX_SOCKET_PATH = 103


def instance_name(model, version):
    """kindle-paperwhite-6 + 5.20.1 → kws-pw6-5201 (instance names allow
    only [a-z0-9_-])."""
    short = model
    for long_form, abbrev in ABBREVIATIONS:
        short = short.replace(long_form, abbrev)
    return re.sub(r"[^a-z0-9_-]", "", f"kws-{short}-{version.replace('.', '')}".lower())


def git(*args, cwd=REPO):
    p = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else None


def cargo_version():
    m = re.search(r'^version = "(.*)"$', (REPO / "Cargo.toml").read_text(), re.M)
    return m.group(1) if m else None


# ---- prerequisites (on the Mac) ----

def prereqs(args, emu):
    out = []

    def add(pid, title, status, detail="", required=True, data=None):
        out.append(result(pid, title, status, detail, required, data))
        log(f"prereq {pid}: {status.upper()}{': ' + detail if detail else ''}")

    root = emu.root
    if not (root / "eink").exists() or not (root / "models.json").exists():
        add("eink_emulator", "eink-emulator checkout", FAIL, f"no eink or models.json in {root}")
        return out
    add("eink_emulator", "eink-emulator checkout", PASS, f"{root} at {git('rev-parse', '--short', 'HEAD', cwd=root)}")

    models = emu.models()
    if args.model not in models:
        kindles = ", ".join(m for m in models if m.startswith("kindle-"))
        add("model", "Model is supported by the emulator", FAIL, f"unknown model {args.model}; Kindles: {kindles}")
        return out
    if not args.model.startswith("kindle-"):
        add("model", "Model is supported by the emulator", FAIL, f"{args.model} isn't a Kindle")
        return out
    add("model", "Model is supported by the emulator", PASS, models[args.model].get("description", ""))

    info = emu.machine_info()
    if info:
        if info.get("model") != args.model:
            add("instance", "Instance matches the model", FAIL,
                f"instance {emu.name} is a {info.get('model')}, not a {args.model}")
            return out
        if args.firmware and info.get("firmware_version") not in (None, args.firmware):
            add("instance", "Instance matches the firmware", FAIL,
                f"instance {emu.name} runs {info.get('firmware_version')}, not {args.firmware}")
            return out
        jb = (info.get("addons") or {}).get("jbsh") or (info.get("addons") or {}).get("jailbreak")
        add("instance", "Instance", PASS if jb else WARN,
            f"using existing instance {emu.name} ({info.get('firmware_version') or 'manual firmware'})"
            + ("" if jb else "; it wasn't created with a jailbreak option"))
    else:
        add("instance", "Instance", PASS, f"will create {emu.name} ({emu.jailbreak_flag()})")
        # The firmware is downloaded on create, or must be supplied by hand.
        if models[args.model].get("ftvdb_package"):
            p = emu.eink("versions", "--model", args.model, timeout=120)
            rows = {line.split()[0]: line.split()[-1] for line in p.stdout.splitlines()[2:] if line.split()}
            if args.firmware not in rows:
                newest = ", ".join(list(rows)[:6])
                add("firmware", "Firmware version is available", FAIL,
                    f"{args.firmware} isn't a known release for {args.model} (newest: {newest})"
                    if p.returncode == 0 else f"./eink versions failed: {p.stderr.strip()[-300:]}")
                return out
            installed = rows[args.firmware] == "installed"
            add("firmware", "Firmware version is available", PASS,
                "already imported" if installed else "will be downloaded (needs network)")
            if not installed and not shutil.which("kindletool"):
                add("kindletool", "KindleTool (to import firmware)", FAIL, "kindletool isn't on PATH")
        else:
            manual = root / "firmware" / args.model
            add("firmware", "Firmware is supplied", PASS if manual.exists() else FAIL,
                f"{args.model} has no download source; using the files in {manual}"
                if manual.exists() else f"{args.model} needs firmware files in {manual} (see docs/firmware.md)")

    sock = emu.machine_dir() / f"{emu.name}.serial.sock"
    if len(str(sock).encode()) > MAX_SOCKET_PATH:
        add("socket_path", "QEMU's socket paths fit", FAIL,
            f"{sock} is {len(str(sock).encode())} bytes; macOS allows {MAX_SOCKET_PATH}. "
            "Use a shorter --instance name")
        return out

    add("ssh_key", "The emulator's SSH key", PASS if emu.key.exists() else FAIL,
        str(emu.key) if emu.key.exists() else f"missing {emu.key}; run ./eink build")

    if args.attach:
        add("running", "The instance is running (--attach)", PASS if emu.running() else FAIL,
            "" if emu.running() else f"{emu.name} isn't running")
    else:
        if emu.running():
            add("running", "The instance isn't already running", FAIL,
                f"{emu.name} is running; stop it, or use --attach")
        busy = [p for p in (emu.ssh_port, emu.wifi_ssh_port) if not port_free(p)]
        add("ports", "Host ports for SSH are free", FAIL if busy else PASS,
            f"in use: {busy}" if busy else f"SSH on {emu.ssh_port}, Wi-Fi SSH on {emu.wifi_ssh_port}")

    # Two host settings break QEMU's DNS proxies, and with them the guest's
    # Wi-Fi (CLAUDE.md, "When the emulator misbehaves").
    usb = subprocess.run(["ifconfig"], capture_output=True, text=True).stdout
    kindle_usb = "inet 192.168.15." in usb
    resolv = Path("/etc/resolv.conf").read_text() if Path("/etc/resolv.conf").exists() else ""
    tailscale = "100.100.100.100" in resolv
    problems = (["a real Kindle is on USB networking (192.168.15.x)"] if kindle_usb else []) + \
               (["Tailscale is managing DNS (100.100.100.100)"] if tailscale else [])
    add("host_network", "Host network leaves the emulator's DNS working", WARN if problems else PASS,
        "; ".join(problems) + ": the Wi-Fi check will likely fail. Unplug the Kindle or quit Tailscale."
        if problems else "no real Kindle on USB; DNS isn't Tailscale's", required=False)

    # The package under test.
    kpkg = REPO / "target" / "package" / "kindle-wireless-sync.kpkg"
    if not args.no_build:
        log("building the package (just package)")
        p = subprocess.run(["just", "package"], cwd=REPO, stdout=sys.stderr, stderr=sys.stderr)
        if p.returncode != 0:
            add("package", "The KPM package builds", FAIL, f"just package exited {p.returncode}")
            return out
    if not kpkg.exists():
        add("package", "The KPM package", FAIL, f"no {kpkg}; run just package")
        return out
    age = time.time() - kpkg.stat().st_mtime
    add("package", "The KPM package", PASS,
        f"{kpkg.relative_to(REPO)}, {kpkg.stat().st_size // 1024} KiB"
        + ("" if not args.no_build or age < 3600 else f"; built {age / 3600:.0f}h ago (--no-build)"))
    return out


# ---- the run ----

def boot(emu, created, args):
    """Boots for testing: with -snapshot, after a one-off jailbreak boot if
    the instance needs one. Returns boot timings."""
    info = emu.machine_info() or {}
    jbsh = bool((info.get("addons") or {}).get("jbsh"))
    timings = {"baseline": False}
    if created and jbsh:
        emu.jailbreak_baseline()
        timings["baseline"] = True
    emu.start(snapshot=not args.no_snapshot)
    timings["ssh_s"] = round(emu.wait_ssh(), 1)
    timings["home_s"] = round(emu.wait_home(), 1)
    if jbsh and not emu.jailbroken() and not timings["baseline"]:
        # An existing --jbsh instance whose first boot never finished.
        log("the instance isn't jailbroken yet")
        emu.stop()
        emu.jailbreak_baseline()
        timings["baseline"] = True
        emu.start(snapshot=not args.no_snapshot)
        timings["ssh_s"] = round(emu.wait_ssh(), 1)
        timings["home_s"] = round(emu.wait_home(), 1)
    return timings


def main():
    ap = argparse.ArgumentParser(
        description="Check a Kindle model and firmware version for compatibility with Kindle Wireless Sync, "
                    "on eink-emulator. Prints one JSON object on stdout; progress on stderr.")
    ap.add_argument("model", nargs="?", help="model id from eink-emulator's models.json, e.g. kindle-paperwhite-6")
    ap.add_argument("firmware", nargs="?", help="firmware version, e.g. 5.20.1 (optional with --instance)")
    ap.add_argument("--instance", help="eink-emulator instance to use (default: kws-<short model>-<version>, "
                                       "created if missing)")
    ap.add_argument("--eink-dir", default=str(REPO.parent / "eink-emulator"), help="eink-emulator checkout")
    ap.add_argument("--ssh-port", type=int, default=4022, help="host port for the guest's USB SSH")
    ap.add_argument("--wifi-ssh-port", type=int, default=4023, help="host port for the guest's Wi-Fi SSH")
    ap.add_argument("--artifacts", help="where screenshots and logs go (default: target/compat/<instance>-<time>)")
    ap.add_argument("--attach", action="store_true",
                    help="use the instance already running on --ssh-port instead of booting it (left running)")
    ap.add_argument("--keep-running", action="store_true", help="leave the emulator running afterwards")
    ap.add_argument("--no-snapshot", action="store_true",
                    help="boot without -snapshot: the checks' changes stay on the instance's disk")
    ap.add_argument("--no-build", action="store_true", help="use the existing target/package/*.kpkg")
    ap.add_argument("--only", help="comma-separated check ids to run")
    ap.add_argument("--skip", help="comma-separated check ids to skip")
    ap.add_argument("--list", action="store_true", help="list the checks and exit")
    args = ap.parse_args()

    if args.list:
        for cid, title, required, _ in checks.CHECKS:
            print(f"{cid:22} {'required' if required else 'optional'}  {title}")
        return 0
    if not args.model:
        ap.error("give a model")
    if not args.firmware and not args.instance:
        ap.error("give a firmware version, or --instance")

    started = datetime.datetime.now(datetime.timezone.utc)
    name = args.instance or instance_name(args.model, args.firmware)
    artifacts = Path(args.artifacts).resolve() if args.artifacts else \
        REPO / "target" / "compat" / f"{name}-{started.strftime('%Y%m%dT%H%M%SZ')}"
    artifacts.mkdir(parents=True, exist_ok=True)
    work = artifacts / "work"
    work.mkdir(exist_ok=True)
    if not args.attach:
        # Another run may hold the default ports: take the next free ones.
        args.ssh_port = free_port(args.ssh_port) if not port_free(args.ssh_port) else args.ssh_port
        args.wifi_ssh_port = free_port(max(args.wifi_ssh_port, args.ssh_port + 1)) \
            if not port_free(args.wifi_ssh_port) or args.wifi_ssh_port == args.ssh_port else args.wifi_ssh_port
    emu = Emulator(args.eink_dir, name, args.model, args.firmware, args.ssh_port, args.wifi_ssh_port, artifacts)
    log(f"checking {args.model} {args.firmware or ''} on instance {name}; artifacts in {artifacts}")

    report = {
        "schema": SCHEMA,
        "started_at": started.isoformat(timespec="seconds"),
        "model": args.model,
        "firmware": args.firmware,
        "instance": name,
        "package": {"version": cargo_version(), "commit": git("rev-parse", "--short", "HEAD"),
                    "dirty": bool(git("status", "--porcelain"))},
        "host": {"platform": platform.platform(), "python": platform.python_version()},
        "artifacts": str(artifacts.relative_to(REPO)) if artifacts.is_relative_to(REPO) else str(artifacts),
    }
    started_mono = time.monotonic()
    code = 2
    try:
        report["description"] = (emu.models().get(args.model) or {}).get("description") \
            if (emu.root / "models.json").exists() else None
        report["prereqs"] = prereqs(args, emu)
        if any(p["required"] and p["status"] == FAIL for p in report["prereqs"]):
            report["outcome"] = "aborted"
            log("a prerequisite failed; not booting")
            return code
        report["eink_emulator_commit"] = git("rev-parse", "--short", "HEAD", cwd=emu.root)

        created = False
        if not emu.machine_info():
            emu.create()
            created = True
        info = emu.machine_info() or {}
        report["firmware"] = report["firmware"] or info.get("firmware_version")
        try:
            if args.attach:
                report["boot"] = {"attached": True}
                if not emu.guest.reachable():
                    raise BootFailed(f"can't SSH to the guest on port {args.ssh_port}")
            else:
                report["boot"] = boot(emu, created, args)
        except BootFailed as e:
            report["outcome"] = "boot_failed"
            report["boot_error"] = str(e)
            log(f"boot failed: {e}")
            return code

        ctx = checks.Context(emu.guest, emu, REPO, REPO / "target" / "package" / "kindle-wireless-sync.kpkg",
                             cargo_version(), artifacts, work)
        only = set(args.only.split(",")) if args.only else None
        skip = set(args.skip.split(",")) if args.skip else set()
        report["checks"] = checks.run_all(ctx, only, skip)
        report["device"] = ctx.state.get("device")
        report["summary"] = summarise(report["checks"])
        report["outcome"] = "completed"
        code = 0 if report["summary"]["compatible"] else 1
        return code
    except KeyboardInterrupt:
        report["outcome"] = "interrupted"
        log("interrupted")
        return 130
    finally:
        if not args.attach and not args.keep_running:
            try:
                emu.stop()
            except Exception as e:  # noqa: BLE001 - still print the report
                log(f"couldn't stop the emulator: {e}")
        elif emu.running():
            log(f"leaving {name} running (SSH on port {args.ssh_port})")
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
        report["duration_s"] = round(time.monotonic() - started_mono)
        (artifacts / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, separators=(",", ":")), flush=True)
        s = report.get("summary")
        if s:
            verdict = "COMPATIBLE" if s["compatible"] else f"NOT COMPATIBLE (failed: {', '.join(s['blocking'])})"
            log(f"{verdict}; {s['counts']}; details in {artifacts}/result.json")
        else:
            log(f"outcome: {report.get('outcome')}; details in {artifacts}/result.json")


if __name__ == "__main__":
    sys.exit(main())
