# SPDX-License-Identifier: GPL-3.0-or-later
"""Development-only Calibre plugin: lets tools/calibre-harness/harness.py drive the GUI (ADR 0019)."""

from calibre.customize import InterfaceActionBase


class KwsHarnessPlugin(InterfaceActionBase):
    name = 'KWS test harness'
    description = 'Development only: a localhost command server for kindle-wireless-sync tests'
    supported_platforms = ['osx', 'linux', 'windows']
    author = 'kindle-wireless-sync'
    version = (0, 1, 0)
    minimum_calibre_version = (9, 16, 0)
    actual_plugin = 'calibre_plugins.kws_harness.action:HarnessAction'
