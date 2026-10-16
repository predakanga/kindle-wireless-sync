#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Turn recorded `kindle-wireless-sync connect --transcript` sessions into shareable fixtures.

Usage: anonymize_transcript.py OUT.jsonl IN.jsonl [IN.jsonl ...]

All inputs are anonymized with one shared mapping, so a library recorded
across several sessions stays consistent. Personal data (titles, authors,
series, identifiers, comments, library names, UUIDs, lpaths, temp paths) is
replaced with generated values and book sizes are shrunk to a few bytes. The
script refuses to write the output if any original value survives.
"""

import json
import re
import sys

UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


class Anonymizer:
    def __init__(self):
        self.uuids = {}
        self.books = {}  # original lpath -> book number
        self.sizes = {}  # original size -> fake size
        self.series = {}
        self.secrets = set()  # original strings that must not survive

    def fake_uuid(self, real):
        if real not in self.uuids:
            self.secrets.add(real)
            self.uuids[real] = f"00000000-0000-4000-8000-{len(self.uuids) + 1:012d}"
        return self.uuids[real]

    def book_number(self, lpath):
        if lpath not in self.books:
            self.secrets.add(lpath)
            self.books[lpath] = len(self.books) + 1
        return self.books[lpath]

    def fake_lpath(self, lpath):
        n = self.book_number(lpath)
        ext = lpath.rsplit(".", 1)[-1] if "." in lpath else "bin"
        return f"Book {n} - Author {n} ({n}).{ext}"

    def fake_size(self, size):
        if size not in self.sizes:
            self.sizes[size] = 32 + len(self.sizes)
        return self.sizes[size]

    def fake_series(self, name):
        if not name:
            return name
        if name not in self.series:
            self.secrets.add(name)
            self.series[name] = f"Series {len(self.series) + 1}"
        return self.series[name]

    def metadata(self, m):
        lpath = m.get("lpath") or ""
        n = self.book_number(lpath)
        for key in ("title", "title_sort", "author_sort", "publisher", "comments"):
            if isinstance(m.get(key), str) and len(m[key]) > 3:
                self.secrets.add(m[key])
        for a in m.get("authors") or []:
            self.secrets.add(a)
        m["title"] = m["title_sort"] = f"Book {n}"
        m["authors"] = [f"Author {n}"]
        m["author_sort"] = f"{n}, Author"
        m["author_sort_map"] = {f"Author {n}": f"{n}, Author"}
        if m.get("series"):
            m["series"] = self.fake_series(m["series"])
        if m.get("_series_sort_"):
            m["_series_sort_"] = self.fake_series(m["_series_sort_"])
        m["comments"] = None
        m["cover"] = None
        m["identifiers"] = {}
        m["publisher"] = "Publisher" if m.get("publisher") else m.get("publisher")
        m["tags"] = []
        m["user_metadata"] = {}
        m["user_categories"] = {}
        m["link_maps"] = {}
        m["application_id"] = n
        # Publication date and page count together can identify a book; the
        # rating is the user's own opinion.
        if m.get("pubdate"):
            m["pubdate"] = "2000-01-01T00:00:00+00:00"
        m["pages"] = None
        m["rating"] = None
        if "thumbnail" in m and m["thumbnail"]:
            m["thumbnail"] = [m["thumbnail"][0], m["thumbnail"][1], "AAAA"]
        if isinstance(m.get("size"), int):
            m["size"] = self.fake_size(m["size"])
        if lpath:
            m["lpath"] = self.fake_lpath(lpath)
        return m

    def walk(self, v, key=None):
        """Generic pass: lpaths, UUIDs and sizes wherever they appear."""
        if isinstance(v, dict):
            out = {}
            for k, x in v.items():
                if k in ("metadata", "data") and isinstance(x, dict):
                    x = self.metadata(x)
                out[k] = self.walk(x, k)
            return out
        if isinstance(v, list):
            return [self.walk(x, key) for x in v]
        if isinstance(v, str):
            if UUID_RE.match(v):
                return self.fake_uuid(v)
            if key in ("lpath", "lpaths") and not v.startswith("Book "):
                return self.fake_lpath(v)
            return v
        if isinstance(v, int) and not isinstance(v, bool) and key in ("length", "fileLength", "size"):
            return self.fake_size(v)
        return v

    def entry(self, e):
        p = e.get("payload")
        if isinstance(p, dict):
            for k in ("currentLibraryName", "libraryName"):
                if k in p:
                    self.secrets.add(p[k])
                    p[k] = "Test Library"
            if "fieldMetadata" in p:
                p["fieldMetadata"] = {}
                p["otherInfo"] = {}
        if e.get("dir") in ("recv-raw", "send-raw"):
            e["len"] = self.fake_size(e["len"])
        return self.walk(e)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    anon = Anonymizer()
    out_lines = []
    for path in sys.argv[2:]:
        out_lines.append(json.dumps({"dir": "session-start", "source": path.rsplit("/", 1)[-1]}))
        with open(path) as f:
            for line in f:
                out_lines.append(json.dumps(anon.entry(json.loads(line)), sort_keys=True))
    text = "\n".join(out_lines) + "\n"
    leaks = sorted(s for s in anon.secrets if s and s in text)
    if leaks:
        sys.exit(f"refusing to write: {len(leaks)} original values survive, e.g. {leaks[0][:40]!r}")
    if re.search(r"/(var/folders|Users|home)/", text):
        sys.exit("refusing to write: a local filesystem path survives")
    with open(sys.argv[1], "w") as f:
        f.write(text)
    print(f"wrote {len(out_lines)} entries, {len(anon.books)} books, {len(anon.uuids)} uuids")


if __name__ == "__main__":
    main()
