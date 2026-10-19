# SPDX-License-Identifier: GPL-3.0-or-later
"""Creating, booting and stopping eink-emulator instances
(../eink-emulator; read its AGENTS.md)."""

import json
import socket
import subprocess
import sys
import time
from pathlib import Path

from common import log
from guest import Guest

# Kindle 4, Touch and Paperwhite 1 can't run jb.sh; they are jailbroken
# offline at build time, without KPM (AGENTS.md, "Match a jailbroken device").
OFFLINE_JAILBREAK = {"kindle-4", "kindle-touch", "kindle-paperwhite-1"}

HOME_APP = "com.lab126.KPPMainApp"


def port_free(port):
    with socket.socket() as s:
        try:
            s.bind(("127.0.0.1", port))
            return True
        except OSError:
            return False


def free_port(preferred):
    """`preferred` if it's free, else the next free one above it."""
    for p in range(preferred, preferred + 200):
        if port_free(p):
            return p
    raise RuntimeError(f"no free port from {preferred}")


class BootFailed(Exception):
    pass


class Emulator:
    def __init__(self, root, name, model, firmware, ssh_port, wifi_ssh_port, logs_dir):
        self.root = Path(root)
        self.name = name
        self.model = model
        self.firmware = firmware
        self.ssh_port = ssh_port
        self.wifi_ssh_port = wifi_ssh_port
        self.logs_dir = Path(logs_dir)
        self.proc = None
        self.boots = 0
        self.key = self.root / "build" / "ssh" / "id_ed25519"
        self.guest = Guest(ssh_port, self.key)

    # ---- inventory ----

    def models(self):
        return json.loads((self.root / "models.json").read_text())

    def machine_dir(self):
        return self.root / "machines" / self.name

    def machine_info(self):
        p = self.machine_dir() / "machine.json"
        return json.loads(p.read_text()) if p.exists() else None

    def eink(self, *args, timeout=None, stream=False):
        """Runs ./eink; with `stream`, its output goes to our stderr."""
        cmd = [str(self.root / "eink"), *args]
        if stream:
            return subprocess.run(cmd, cwd=self.root, stdout=sys.stderr, stderr=sys.stderr, timeout=timeout)
        return subprocess.run(cmd, cwd=self.root, capture_output=True, text=True, timeout=timeout)

    def qmp(self, *args, timeout=30):
        return subprocess.run(
            [sys.executable, str(self.root / "scripts" / "eink-qmp.py"), "--machine", self.name, *args],
            cwd=self.root, capture_output=True, text=True, timeout=timeout,
        )

    def jailbreak_flag(self):
        return "--jailbreak" if self.model in OFFLINE_JAILBREAK else "--jbsh"

    def create(self):
        model = self.models()[self.model]
        args = ["create", self.name, "--model", self.model, self.jailbreak_flag()]
        if model.get("device_profiles"):
            args += ["--profile", "production"]
        if self.firmware:
            args += ["--firmware", self.firmware]
        log(f"creating instance {self.name}: ./eink {' '.join(args)}")
        p = self.eink(*args, stream=True, timeout=3600)
        if p.returncode != 0:
            raise BootFailed(f"./eink create failed (exit {p.returncode}); see the output above")

    # ---- running ----

    def running(self):
        """Whether this instance's QEMU answers on its QMP socket."""
        if not (self.machine_dir() / f"{self.name}.qmp.sock").exists():
            return False
        try:
            return self.qmp("status", timeout=10).returncode == 0
        except subprocess.TimeoutExpired:
            return False

    def start(self, snapshot):
        self.boots += 1
        out = self.logs_dir / f"qemu-boot{self.boots}.log"
        args = [str(self.root / "eink"), "run", self.name, "--headless", "--serial-socket", "--qmp-socket",
                "--ssh-port", str(self.ssh_port), "--wifi-ssh-port", str(self.wifi_ssh_port)]
        if snapshot:
            args += ["--", "-snapshot"]
        log(f"booting {self.name}{' (snapshot: guest changes are discarded)' if snapshot else ''}; "
            f"QEMU output in {out}")
        self.snapshot = snapshot
        self.proc = subprocess.Popen(args, cwd=self.root, stdout=out.open("w"), stderr=subprocess.STDOUT)

    def alive(self):
        return self.proc is not None and self.proc.poll() is None

    def serial_tail(self, lines=15):
        p = self.machine_dir() / f"{self.name}.serial.log"
        if not p.exists():
            return ""
        return "\n".join(p.read_text(errors="replace").splitlines()[-lines:])

    def wait_ssh(self, timeout=300):
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            if self.proc is not None and not self.alive():
                raise BootFailed(f"QEMU exited (code {self.proc.returncode}) before SSH answered.\n"
                                 f"Serial log tail:\n{self.serial_tail()}")
            if self.guest.reachable():
                took = time.monotonic() - start
                log(f"SSH answers after {took:.0f}s")
                return took
            time.sleep(4)
        raise BootFailed(f"SSH didn't answer within {timeout}s. Serial log tail:\n{self.serial_tail()}")

    def wait_home(self, timeout=420):
        """Waits for the framework: Home in the foreground and the catalog's
        session token written."""
        took = self.guest.wait_for(
            f"[ \"$(lipc-get-prop com.lab126.appmgrd activeApp 2>/dev/null)\" = {HOME_APP} ] "
            "&& [ -s /tmp/session_token ]",
            timeout=timeout, interval=5)
        if took is None:
            app = self.guest.run("lipc-get-prop com.lab126.appmgrd activeApp")[1].strip()
            raise BootFailed(f"Home didn't come up within {timeout}s (activeApp: {app or 'none'})")
        log(f"Home is up after another {took:.0f}s")
        return took

    def boot_id(self):
        return self.guest.run("cat /proc/sys/kernel/random/boot_id", timeout=15)[1].strip()

    def stop(self, sync=False):
        if sync and self.guest.reachable():
            self.guest.run("sync", timeout=60)
        if self.running():
            log(f"stopping {self.name}")
            self.qmp("quit", timeout=30)
        if self.proc is not None:
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None

    def delete(self):
        """Removes the instance, and any base image no other instance uses
        (`./eink delete`). The instance must be stopped."""
        log(f"removing instance {self.name}")
        p = self.eink("delete", self.name, timeout=300)
        for line in (p.stdout + p.stderr).strip().splitlines():
            log(f"  {line}")
        if p.returncode != 0:
            raise RuntimeError(f"./eink delete {self.name} exited {p.returncode}")

    # ---- jailbreak baseline ----

    def jailbroken(self):
        return self.guest.ok("[ -x /var/local/kmc/bin/kpm ] && [ -f /mnt/us/documents/JAILBROKEN.txt ]")

    def jailbreak_baseline(self, timeout=1200):
        """First boot of a --jbsh instance, without -snapshot: jb.sh runs
        once Home is reached, then reboots the guest. QEMU follows the reboot
        on some models and exits on others (AGENTS.md). Ends with the
        instance stopped and its disk jailbroken."""
        log("first boot: letting jb.sh jailbreak the instance (about 7 minutes on a Paperwhite 6)")
        self.start(snapshot=False)
        deadline = time.monotonic() + timeout
        self.wait_ssh()
        first = self.boot_id()
        while time.monotonic() < deadline:
            if not self.alive():
                log("QEMU exited after the jailbreak's reboot; starting it again")
                self.start(snapshot=False)
                self.wait_ssh()
                break
            if self.guest.reachable():
                bid = self.boot_id()
                if bid and bid != first:
                    log("guest rebooted after the jailbreak")
                    break
            time.sleep(10)
        else:
            raise BootFailed(f"jb.sh didn't reboot the guest within {timeout}s; "
                             "see /var/local/eink-addons/jbsh.log in the guest")
        self.wait_home()
        if not self.jailbroken():
            jblog = self.guest.run("tail -20 /var/local/eink-addons/jbsh.log")[1]
            raise BootFailed(f"the instance isn't jailbroken after jb.sh:\n{jblog}")
        log("jailbroken; saving the baseline")
        self.stop(sync=True)
