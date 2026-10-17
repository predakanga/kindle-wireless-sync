#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Launch an isolated Calibre with the KWS test harness plugin and send it commands (ADR 0019).

Usage: harness.py COMMAND [ARGS]

  launch [--library DIR]        start Calibre with config, caches and library in dev-calibre/
  quit [--force]                quit it (refuses during device jobs unless --force)
  status                        library, wireless server, device, jobs and open dialogs
  wireless-start [--port N] [--password P]
  wireless-stop
  wait-device [--timeout S]     wait for a connected device with no jobs left
  device-books                  the device's book list as Calibre sees it
  dialogs [--dismiss]           visible dialogs (error dialogs included)
  exec CODE | exec -            run Python in the GUI (gui, db, dm in scope); prints `result`

Output is JSON. The exit status is 1 when a command fails, and for
wait-device when it times out or a dialog opens.
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEV = REPO / 'dev-calibre'
CONFIG = DEV / 'config'
CACHE = DEV / 'cache'
LOG = DEV / 'calibre.log'
STATE = CONFIG / 'kws-harness.json'
PLUGIN = Path(__file__).resolve().parent / 'plugin'
CALIBRE_BIN = Path(os.environ.get('CALIBRE_BIN', '/Applications/calibre.app/Contents/MacOS'))


class Failed(Exception):
    pass


def calibre_env():
    env = dict(os.environ)
    env.update(CALIBRE_CONFIG_DIRECTORY=str(CONFIG), CALIBRE_CACHE_DIRECTORY=str(CACHE), KWS_HARNESS='1')
    return env


def calibre_running():
    """True if any Calibre GUI is running. Its single-instance socket is per user, not per config."""
    if sys.platform != 'darwin':
        raise Failed('only macOS is supported (the GUI socket path differs elsewhere)')
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(f'/tmp/calibre-{os.getuid()}-gui.sock')
        return True
    except OSError:
        return False
    finally:
        s.close()


def call(command, **args):
    try:
        state = json.loads(STATE.read_text())
    except FileNotFoundError:
        raise Failed('the harness is not running (start it with: harness.py launch)')
    req = urllib.request.Request(
        f'http://127.0.0.1:{state["port"]}/{command}',
        data=json.dumps(args).encode(),
        headers={'Authorization': f'Bearer {state["token"]}', 'Content-Type': 'application/json'},
        method='POST',
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(req, timeout=90) as r:
            body = json.load(r)
    except urllib.error.HTTPError as e:
        body = json.load(e)
    except urllib.error.URLError as e:
        raise Failed(f'the harness is not answering ({e.reason}); is its Calibre still running?')
    if not body['ok']:
        raise Failed(body['error'])
    return body['result']


def launch(library, timeout):
    if calibre_running():
        raise Failed("Calibre is already running. Quit it first: the harness won't quit a Calibre it didn't start.")
    CONFIG.mkdir(parents=True, exist_ok=True)
    CACHE.mkdir(parents=True, exist_ok=True)
    STATE.unlink(missing_ok=True)
    env = calibre_env()
    subprocess.run([CALIBRE_BIN / 'calibre-customize', '-b', PLUGIN], env=env, check=True)
    subprocess.run(
        [CALIBRE_BIN / 'calibre-debug', '-c',
         "from calibre.utils.config import dynamic; dynamic.set('welcome_wizard_was_run', True)"],
        env=env, check=True,
    )
    with open(LOG, 'ab') as log:
        proc = subprocess.Popen(
            [CALIBRE_BIN / 'calibre-debug', '-g', '--', '--with-library', str(library), '--no-update-check'],
            env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
        )
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise Failed(f'Calibre exited with status {proc.returncode}; see {LOG}')
        if STATE.exists():
            try:
                return call('status')
            except Failed:
                pass
        time.sleep(1)
    raise Failed(f'the harness did not come up within {timeout} s; see {LOG}')


def wait_device(timeout):
    deadline = time.monotonic() + timeout
    while True:
        status = call('status')
        if status['dialogs']:
            raise Failed('a dialog opened while waiting:\n' + json.dumps(status['dialogs'], indent=2))
        device = status['device']
        if device and not device['ejected'] and not status['jobs']:
            return status
        if time.monotonic() >= deadline:
            raise Failed(f'no idle device after {timeout} s; last status:\n' + json.dumps(status, indent=2))
        time.sleep(1)


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest='command', required=True)
    c = sub.add_parser('launch')
    c.add_argument('--library', type=Path, default=DEV / 'library')
    c.add_argument('--timeout', type=float, default=90)
    sub.add_parser('quit').add_argument('--force', action='store_true')
    sub.add_parser('status')
    c = sub.add_parser('wireless-start')
    c.add_argument('--port', type=int)
    c.add_argument('--password')
    sub.add_parser('wireless-stop')
    sub.add_parser('wait-device').add_argument('--timeout', type=float, default=120)
    sub.add_parser('device-books')
    sub.add_parser('dialogs').add_argument('--dismiss', action='store_true')
    sub.add_parser('exec').add_argument('code', help="Python code, or - to read it from stdin")
    a = p.parse_args()

    try:
        if a.command == 'launch':
            result = launch(a.library.resolve(), a.timeout)
        elif a.command == 'quit':
            result = call('quit', force=a.force)
        elif a.command == 'wireless-start':
            args = {k: v for k, v in (('port', a.port), ('password', a.password)) if v is not None}
            result = call('wireless-start', **args)
        elif a.command == 'wait-device':
            result = wait_device(a.timeout)
        elif a.command == 'dialogs':
            result = call('dialogs', dismiss=a.dismiss)
        elif a.command == 'exec':
            result = call('exec', code=sys.stdin.read() if a.code == '-' else a.code)
        else:
            result = call(a.command)
    except Failed as e:
        print(e, file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
