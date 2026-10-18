# SPDX-License-Identifier: GPL-3.0-or-later
"""A minimal PNG reader (8-bit greyscale, RGB or RGBA, not interlaced):
enough to read QEMU's screendumps without third-party modules."""

import struct
import zlib


def read_grey(path):
    """Returns (width, height, rows): one bytes object of 0-255 luma per
    row."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos, idat, width = 8, [], None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour, _, _, interlace = struct.unpack(">IIBBBBB", chunk)
            if depth != 8 or interlace:
                raise ValueError(f"{path}: unsupported PNG (depth {depth}, interlace {interlace})")
            channels = {0: 1, 2: 3, 4: 2, 6: 4}[colour]
        elif kind == b"IDAT":
            idat.append(chunk)
        elif kind == b"IEND":
            break
        pos += 12 + length
    raw = zlib.decompress(b"".join(idat))
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if f == 1:
                line[i] = (line[i] + a) & 0xFF
            elif f == 2:
                line[i] = (line[i] + b) & 0xFF
            elif f == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 0xFF
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        prev = line
        if channels == 1 or channels == 2:
            rows.append(bytes(line[::channels]))
        else:
            rows.append(bytes((line[i] * 299 + line[i + 1] * 587 + line[i + 2] * 114) // 1000
                              for i in range(0, stride, channels)))
    return width, height, rows
