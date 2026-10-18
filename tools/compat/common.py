# SPDX-License-Identifier: GPL-3.0-or-later
"""Shared pieces of the compatibility harness: logging to stderr and check
results."""

import sys
import time

PASS, FAIL, WARN, SKIP, ERROR = "pass", "fail", "warn", "skip", "error"

_start = time.monotonic()


def log(msg):
    """Progress for the person watching: stderr, with elapsed time."""
    t = time.monotonic() - _start
    print(f"[{int(t // 60):3d}:{int(t % 60):02d}] {msg}", file=sys.stderr, flush=True)


class CheckFailed(Exception):
    """Raised inside a check to end it with a status and a reason."""

    def __init__(self, status, detail, data=None):
        super().__init__(detail)
        self.status = status
        self.detail = detail
        self.data = data or {}


def result(check_id, title, status, detail="", required=True, data=None, duration=None):
    out = {"id": check_id, "title": title, "status": status, "required": required, "detail": detail}
    if data:
        out["data"] = data
    if duration is not None:
        out["duration_s"] = round(duration, 1)
    return out


def summarise(checks):
    counts = {}
    for c in checks:
        counts[c["status"]] = counts.get(c["status"], 0) + 1
    blocking = [c["id"] for c in checks if c["required"] and c["status"] in (FAIL, ERROR)]
    return {"counts": counts, "blocking": blocking, "compatible": not blocking}
