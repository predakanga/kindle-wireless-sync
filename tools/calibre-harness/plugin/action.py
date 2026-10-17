# SPDX-License-Identifier: GPL-3.0-or-later
"""The harness's command server, running inside Calibre's GUI process (ADR 0019).

An HTTP server on 127.0.0.1 takes `POST /<command>` with a JSON object of
arguments and a bearer token. Each command runs on the GUI thread, passed
over with a queued signal, and the reply is `{"ok": true, "result": ...}` or
`{"ok": false, "error": "..."}`. The port and token are in `kws-harness.json`
in Calibre's config directory. Nothing starts unless KWS_HARNESS=1.
"""

import datetime
import hmac
import json
import os
import secrets
import threading
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from qt.core import QApplication, QDialog, QMessageBox, QObject, Qt, QTimer, pyqtSignal

from calibre.constants import config_dir
from calibre.gui2.actions import InterfaceAction

STATE_FILE = os.path.join(config_dir, 'kws-harness.json')
# How long a request waits for the GUI thread before giving up.
GUI_TIMEOUT = 60


class HarnessError(Exception):
    """A command failed in a way the caller should see as a plain message."""


class _Call:
    def __init__(self, fn):
        self.fn = fn
        self.done = threading.Event()
        self.result = None
        self.error = None


class _GuiDispatcher(QObject):
    """Runs callables on the thread this object lives on (the GUI thread)."""

    call = pyqtSignal(object)

    def __init__(self):
        super().__init__()
        self.call.connect(self._run, type=Qt.ConnectionType.QueuedConnection)

    def _run(self, c):
        try:
            c.result = c.fn()
        except HarnessError as e:
            c.error = str(e)
        except Exception:
            c.error = traceback.format_exc()
        finally:
            c.done.set()

    def run(self, fn):
        c = _Call(fn)
        self.call.emit(c)
        if not c.done.wait(GUI_TIMEOUT):
            raise HarnessError(f'the GUI thread did not answer within {GUI_TIMEOUT} s')
        if c.error is not None:
            raise HarnessError(c.error)
        return c.result


def _to_json(value):
    if isinstance(value, (datetime.datetime, datetime.date)):
        return value.isoformat()
    if isinstance(value, (set, frozenset, tuple)):
        return list(value)
    return repr(value)


def _visible_dialogs():
    """(widget, description) for every visible dialog, error dialogs included."""
    out = []
    for w in QApplication.topLevelWidgets():
        if not isinstance(w, QDialog) or not w.isVisible():
            continue
        d = {'class': type(w).__name__, 'title': w.windowTitle()}
        if isinstance(w, QMessageBox):
            d['text'] = w.text()
            d['details'] = w.detailedText()
        else:
            # Calibre's MessageBox (error_dialog, warning_dialog, ...).
            msg = getattr(w, 'msg', None)
            if hasattr(msg, 'text'):
                d['text'] = msg.text()
            det = getattr(w, 'det_msg', None)
            if hasattr(det, 'toPlainText'):
                d['details'] = det.toPlainText()
        out.append((w, d))
    return out


class HarnessAction(InterfaceAction):
    name = 'KWS test harness'
    action_spec = ('KWS test harness', None, 'Development only', None)
    dont_add_to = InterfaceAction.all_locations
    action_type = 'global'

    def genesis(self):
        self.server = None
        if os.environ.get('KWS_HARNESS') != '1':
            return
        self.dispatcher = _GuiDispatcher()
        self.token = secrets.token_hex(16)
        self.commands = {
            'status': self.status,
            'wireless-start': self.wireless_start,
            'wireless-stop': self.wireless_stop,
            'device-books': self.device_books,
            'dialogs': self.dialogs,
            'exec': self.exec_code,
            'quit': self.quit,
        }
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self._handler_class())
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, name='KwsHarness', daemon=True).start()
        self._write_state(self.server.server_address[1])
        print('KWS harness listening on 127.0.0.1:%d' % self.server.server_address[1], flush=True)

    def shutting_down(self):
        if self.server is None:
            return
        self.server.shutdown()
        self.server.server_close()
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass

    def _write_state(self, port):
        tmp = STATE_FILE + '.tmp'
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as f:
            json.dump({'port': port, 'token': self.token, 'pid': os.getpid()}, f)
        os.replace(tmp, STATE_FILE)

    def _handler_class(self):
        harness = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                expected = f'Bearer {harness.token}'.encode()
                given = self.headers.get('Authorization', '').encode()
                if not hmac.compare_digest(given, expected):
                    return self._reply(403, {'ok': False, 'error': 'bad token'})
                command = harness.commands.get(self.path.strip('/'))
                if command is None:
                    return self._reply(404, {'ok': False, 'error': f'unknown command {self.path!r}'})
                try:
                    length = int(self.headers.get('Content-Length') or 0)
                    args = json.loads(self.rfile.read(length) or b'{}')
                    if not isinstance(args, dict):
                        raise HarnessError('arguments must be a JSON object')
                    result = harness.dispatcher.run(lambda: command(**args))
                except (HarnessError, ValueError) as e:
                    return self._reply(500, {'ok': False, 'error': str(e)})
                self._reply(200, {'ok': True, 'result': result})

            def _reply(self, code, body):
                data = json.dumps(body, default=_to_json).encode()
                self.send_response(code)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        return Handler

    # Commands. Each runs on the GUI thread and must not wait for anything.

    def _wireless(self):
        dm = self.gui.device_manager
        running = dm.is_running('smartdevice')
        driver = dm.dynamic_plugins.get('smartdevice')
        return {
            'enabled': dm.is_enabled('smartdevice'),
            'running': running,
            'port': getattr(driver, 'port', None) if running else None,
            'fixed_port': bool(dm.get_option('smartdevice', 'use_fixed_port')),
            'password_set': bool(dm.get_option('smartdevice', 'password')),
        }

    def status(self):
        gui = self.gui
        dm = gui.device_manager
        db = gui.current_db
        device = None
        if dm.is_device_connected:
            dev = dm.device
            device = {'name': dev.get_gui_name(), 'driver': type(dev).__name__, 'ejected': not dm.is_device_present}
        jobs = gui.job_manager.unfinished_jobs()
        return {
            'library': {'path': db.library_path, 'uuid': db.library_id, 'books': len(db.new_api.all_book_ids())},
            'wireless': self._wireless(),
            'device': device,
            'jobs': [{'description': j.description, 'running': j.is_running} for j in jobs],
            'dialogs': [d for _, d in _visible_dialogs()],
        }

    def wireless_start(self, port=None, password=None):
        """Start the wireless server. A port or password given here is saved, as the GUI's dialog does."""
        dm = self.gui.device_manager
        if not dm.is_enabled('smartdevice'):
            raise HarnessError('the wireless device driver is disabled')
        if not dm.is_running('smartdevice'):
            if password is not None:
                dm.set_option('smartdevice', 'password', str(password))
            if port is not None:
                dm.set_option('smartdevice', 'use_fixed_port', True)
                dm.set_option('smartdevice', 'port_number', str(int(port)))
            message = dm.start_plugin('smartdevice')
            self.gui.iactions['Connect Share'].set_smartdevice_action_state()
            if not dm.is_running('smartdevice'):
                raise HarnessError(f'the wireless server did not start: {message}')
        return self._wireless()

    def wireless_stop(self):
        dm = self.gui.device_manager
        if dm.is_running('smartdevice'):
            dm.stop_plugin('smartdevice')
            self.gui.iactions['Connect Share'].set_smartdevice_action_state()
        return self._wireless()

    def device_books(self):
        """The main memory book list as Calibre sees it, with how each book matched the library."""
        if not self.gui.device_manager.is_device_connected:
            raise HarnessError('no device is connected')
        out = []
        for book in self.gui.booklists()[0]:
            out.append({
                'lpath': getattr(book, 'lpath', None),
                'title': getattr(book, 'title', None),
                'authors': list(getattr(book, 'authors', None) or []),
                'uuid': getattr(book, 'uuid', None),
                'size': getattr(book, 'size', None),
                'last_modified': getattr(book, 'last_modified', None),
                'in_library': getattr(book, 'in_library', None),
                'library_id': getattr(book, 'application_id', None),
            })
        return out

    def dialogs(self, dismiss=False):
        """Visible dialogs. With dismiss, each is rejected (Cancel/No/Close) after listing."""
        found = _visible_dialogs()
        if dismiss:
            for w, _ in found:
                w.reject()
        return [d for _, d in found]

    def exec_code(self, code):
        """Run Python with gui, db (new_api) and dm in scope; return the value of `result`."""
        ns = {'gui': self.gui, 'db': self.gui.current_db.new_api, 'dm': self.gui.device_manager, 'result': None}
        exec(compile(code, '<harness>', 'exec'), ns)
        return ns['result']

    def quit(self, force=False):
        if self.gui.job_manager.has_device_jobs(queued_also=True) and not force:
            raise HarnessError('device jobs are running; pass force to quit anyway')
        # Reply first, then quit.
        QTimer.singleShot(200, lambda: self.gui.quit(confirm_quit=False))
        return 'quitting'
