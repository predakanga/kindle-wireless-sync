# SPDX-License-Identifier: GPL-3.0-or-later
"""SSH access to the emulated Kindle."""

import shlex
import subprocess
import time


class Guest:
    def __init__(self, port, key, host="127.0.0.1"):
        self.port = port
        self.host = host
        # Written out rather than relying on ~/.ssh/config, so any instance
        # and port works.
        self.opts = [
            "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR",
            "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none",
            "-o", "ServerAliveInterval=10", "-i", str(key), "-p", str(port),
        ]

    def ssh_args(self, *extra):
        return ["ssh", *self.opts, *extra, f"root@{self.host}"]

    def run(self, script, timeout=60, stdin=None, check=False):
        """Runs `script` with the guest's sh. Returns (rc, stdout, stderr);
        rc is None on a timeout."""
        try:
            p = subprocess.run(
                self.ssh_args("-o", "ConnectTimeout=5") + [script],
                input=stdin, capture_output=True, timeout=timeout,
            )
        except subprocess.TimeoutExpired:
            return None, "", f"timed out after {timeout}s"
        out = p.stdout.decode("utf-8", "replace")
        err = p.stderr.decode("utf-8", "replace")
        if check and p.returncode != 0:
            raise RuntimeError(f"guest command failed ({p.returncode}): {script}\n{err.strip()}")
        return p.returncode, out, err

    def out(self, script, timeout=60):
        """stdout of `script`, stripped; raises if it fails."""
        return self.run(script, timeout=timeout, check=True)[1].strip()

    def ok(self, script, timeout=60):
        return self.run(script, timeout=timeout)[0] == 0

    def reachable(self):
        try:
            p = subprocess.run(
                self.ssh_args("-o", "ConnectTimeout=3") + ["true"],
                capture_output=True, timeout=15,
            )
            return p.returncode == 0
        except subprocess.TimeoutExpired:
            return False

    def put(self, data, remote, mode=None):
        """Writes bytes to a guest file. The guest has no SFTP, and
        `cat >` is what works reliably (CLAUDE.md)."""
        script = f"cat > {shlex.quote(remote)}"
        if mode:
            script += f" && chmod {mode} {shlex.quote(remote)}"
        self.run(script, stdin=data, timeout=120, check=True)

    def put_tree(self, local_dir, remote_dir):
        """Copies a local directory's contents into `remote_dir` (tar)."""
        tar = subprocess.run(["tar", "-C", str(local_dir), "-cf", "-", "."], capture_output=True, check=True)
        self.run(f"mkdir -p {shlex.quote(remote_dir)} && tar -C {shlex.quote(remote_dir)} -xf -",
                 stdin=tar.stdout, timeout=120, check=True)

    def reverse_tunnel(self, guest_port, host_port):
        """Forwards guest 127.0.0.1:guest_port to host 127.0.0.1:host_port
        for as long as the returned process runs."""
        p = subprocess.Popen(
            self.ssh_args("-N", "-o", "ExitOnForwardFailure=yes",
                          "-R", f"{guest_port}:127.0.0.1:{host_port}"),
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        time.sleep(2)
        if p.poll() is not None:
            raise RuntimeError("SSH tunnel failed: " + p.stderr.read().decode("utf-8", "replace").strip())
        return p

    def wait_for(self, script, timeout, interval=2):
        """Polls until `script` succeeds; returns the seconds it took, or
        None at the deadline."""
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            if self.ok(script, timeout=30):
                return time.monotonic() - start
            time.sleep(interval)
        return None

    def sql_write(self, db, script, attempts=10):
        """Runs SQL statements against `db`, retried while it's locked.
        Only for appreg.db: never write cc.db or ksdk.asset.db (ADR 0005)."""
        assert "cc.db" not in db and "ksdk" not in db
        err = ""
        for _ in range(attempts):
            rc, _, err = self.run(f"sqlite3 {shlex.quote(db)}", stdin=script.encode(), timeout=30)
            if rc == 0:
                return
            if "locked" not in err and "busy" not in err:
                break
            time.sleep(1)
        raise RuntimeError(f"sqlite3 {db}: {err.strip()}")

    def sql(self, db, query, attempts=8):
        """A read-only sqlite3 query, retried while the database is locked
        (the scanner holds cc.db while it indexes, CLAUDE.md)."""
        last = ""
        for _ in range(attempts):
            rc, out, err = self.run(f"sqlite3 -readonly {shlex.quote(db)} {shlex.quote(query)}", timeout=30)
            if rc == 0:
                return out.strip()
            last = err.strip()
            if "locked" not in last and "busy" not in last:
                break
            time.sleep(1)
        raise RuntimeError(f"sqlite3 {db}: {last}")
