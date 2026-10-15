# 0007: Accept every format the built-in reader opens

- Status: Accepted
- Date: 2026-10-15

## Context
At the handshake, the device sends `acceptedExtensions`, which becomes
Calibre's format list for it. Against Calibre 9.16.0 we observed that Calibre
sends **the first listed format the book already has**, and converts only
when the book has none of them. So a book with MOBI and EPUB arrives as MOBI,
even with `azw3` listed first. Dropping `mobi` would force conversion to AZW3,
the better format. See `docs/research/calibre-wireless-protocol.md`
("Observed against Calibre 9.16.0").

## Decision
Accept every sideload format the built-in reader opens, in this order of
preference: `azw3`, `mobi`, `azw`, `kfx`, `pdf`, `txt`. If the user wants a
MOBI book upgraded to AZW3, they convert it in Calibre. We don't force it.
EPUB stays excluded, because the reader ignores sideloaded EPUBs (probed on
5.20.1), so Calibre always converts those.

## Alternatives considered
- Dropping `mobi` to force AZW3: rejected. Format choice belongs to the user.
  It would also make every send of a MOBI-only book wait for a conversion.

## Consequences
- MOBI books keep MOBI's limits (older layout features). Users who care
  convert them in Calibre.
- KFX is accepted for users who have Calibre's KFX Output plugin. Stock
  Calibre can't produce KFX, so this changes nothing for others. Phase 3
  needs cover/`cdeKey` handling for KFX as well as MOBI/AZW3 (ADR 0005).
