# B1 review: fit, licences and reproducibility

Branch `feat/boards` at `7f52913`. This review is read-only: no product code was changed.

**Verdict: FAIL.** Three Important findings, each with a small fix:

1. Seven bundled notices are missing.
2. The fractional-indexing shim is credited as original work.
3. New throws away an unsaved untitled board.

The bundle rebuilds byte for byte, every licence is on the allowed list, and the board costs nothing until it
opens. Once the three Important findings are fixed, this branch would pass on this lens.

## What was checked, and how

| Check | Result |
|---|---|
| `vendor-src/excalidraw/build.sh` in a scratch copy of `7f52913` (Node 22.22.0, npm 10.9.4) | `npm ci` installed 263 packages. The build printed 10006 fractional-indexing comparisons, 82 packages, 236 files, 16,474,429 bytes. |
| `diff -r` of the rebuilt `web/vendor/excalidraw/` against the committed folder | Identical, byte for byte. |
| `sha256sum -c web/vendor/excalidraw/SHA256SUMS` | All 236 OK. |
| The 236 sums listed in `web/vendor/README.md` against `SHA256SUMS` | Identical. model-viewer's sum matches too. |
| Licences in `LICENSES.txt` | 79 MIT, 2 Apache-2.0 (browser-fs-access, crc-32), 1 0BSD (tslib). |
| Code inlined in prebuilt files (not seen by the build's package scan) | 7 more packages, all MIT or ISC. Their notices are missing (finding 1). |
| Font name tables (fontTools) | Match the notes in `build.mjs`. Comic Shanns carries its full MIT text inside the font. |
| My own fractional-indexing comparison | 137,992 comparisons: about 40,000 ordered pairs with n up to 49, 40,000 malformed keys, and three custom digit sets. No differences in results or error messages. |
| pako shim against pako 2.0.3 | 204 round trips: shim deflate to pako inflate, pako deflate and gzip to shim inflate, Unicode up to U+2FFFF. No failures. |
| `cargo test --release` (scratch copy) | 174 passed, 0 failed, 2 ignored, as the report says. |
| Headless Chromium against a test-mode gateway (`AGAVE_FAKE_PROVIDERS=1`, port 4199) | See "Size and load" and finding 3. No off-site requests. No console errors from the board. |

## Findings

### Important

**1. Seven bundled packages' notices are missing from `LICENSES.txt`**
(`vendor-src/excalidraw/build.mjs:201`, also `:156`)

The build lists licences by package folder from esbuild's metafile. It misses code that other packages ship already
inlined:

- `@excalidraw/excalidraw/dist/prod/chunk-EIO257PC.js` embeds two WebAssembly modules:
  - fonteditor-core 2.4.0: MIT, "Copyright (c) 2014 ecomfe".
  - harfbuzzjs 0.3.6: MIT, "Copyright (c) 2019 Ebrahim Byagowi". Its notice also names Apache-2.0 code
    (Zephyr's `zephyr-string.c`) and emscripten's `emmalloc.cpp`.

  `LICENSES.txt` has HarfBuzz's "Old MIT" text and Google's woff2 notice. It has neither of these two.
- `pica/dist/pica.js` is a browserify bundle that inlines five packages. `image-blob-reduce`'s ESM build inlines
  pica again. The five are:
  - glur 1.1.2 (MIT)
  - multimath 2.0.0 (MIT)
  - object-assign 4.1.1 (MIT)
  - webworkify 1.5.0 (MIT)
  - inherits 2.0.4 (ISC)

`legalComments: "none"` also strips the notices Excalidraw itself put in the chunk. So `excalidraw.js` and
`LICENSES.txt` together leave out copyright notices that MIT and ISC require. This is in a public repo, and the
report and `web/vendor/README.md` both say "every notice is in LICENSES.txt".

Scenario: anyone who redistributes the studio, or audits it, finds MIT and ISC code from seven projects with no
notice.

Smallest fix:

- Add the seven upstream licence texts to `vendor-src/excalidraw/licenses/`.
- Append them in the "Compiled into…" block at `build.mjs:201-202`, next to `harfbuzz.txt` and `woff2.txt`.
- Rebuild, then update `SHA256SUMS` and the README list. Only `LICENSES.txt` changes.
- Optionally, have the build fail when a dist file contains `taken from the package "` or a browserify module map
  that names a package it has no notice for.

**2. The fractional-indexing shim is credited as original work** (`vendor-src/excalidraw/shims/fractional-indexing.js:1-7`)

The header reads "Copyright 2026 LayerMaker LLC… Agave's own implementation of the same scheme". The code is the
original package's `src/index.js` with the identifiers renamed:

- `midpoint` became `between`.
- `validateOrderKey` became `checkKey`.
- `incrementInteger` and `decrementInteger` became `increment` and `decrement`.

The structure, the branches and the error strings are the same, which is why the comparisons find no difference.
CC0 allows this, so nothing is illegal. But the authorship claim is wrong in a public repo, and the decision the
report puts to the owner ("CC0 is outside the list, so I replaced it") is mis-framed: CC0-derived code ships either
way. Related: tslib is 0BSD, also a no-attribution licence, and the build accepts it as "BSD"
(`build.mjs:22`). So the allowed list already admits a CC0-like licence.

Scenario: a reader trusts the header, or the owner keeps the shim believing it removes CC0 code from the bundle.

Smallest fix, either of:

- Credit the source in the header: "Adapted from fractional-indexing 3.2.0 (David Greenspan, CC0-1.0), based on
  observablehq.com/@dgreensp/implementing-fractional-indexing".
- Or accept CC0 next to 0BSD, drop the alias at `build.mjs:56`, and ship the original with its CC0 notice.

Fix the report's "Licence decision for you" to match whichever you choose.

**3. New throws away an unsaved untitled board** (`web/app/board.js:213`, also `:149-156` and `:321`)

`newBoard()` calls `keepCurrent()`, which stores the current scene under the name `''`. Then it calls
`kept.delete(UNTITLED)`, which deletes what was just stored. Reproduced in headless Chromium:

- Draw on an untitled board. The state reads "Untitled board · unsaved changes". Press New. The state reads
  "Untitled board" and the Open list has no "Untitled board (unsaved)" entry. The drawing is gone.
- Draw on an untitled board, then open a saved board. The list shows "Untitled board (unsaved)". Press New. That
  entry disappears.

No prompt appears in either case. `beforeunload` stops asking too, because nothing counts as unsaved any more. A
second path, from reading the code: after Delete, the drawing becomes `cur.name = ''` (`:321`). If an untitled board
was already kept, the next switch overwrites `kept['']` with the deleted board's drawing and loses the earlier one.

This contradicts:

- the New button's own title ("unsaved changes on this one are kept"),
- the report ("They also survive opening another board or a new one"),
- the CHANGELOG ("a board switched away from keeps its unsaved changes").

The report's browser check never pressed New on a drawn untitled board.

Smallest fix:

- In `newBoard()`, remove `kept.delete(UNTITLED)`.
- In `keepCurrent()`, when `cur.name === ''` and `kept` already holds `''`, store the scene under a fresh key
  (for example `'' + n`) instead of overwriting it.
- If several untitled boards are too much, have New ask first when the current untitled board is dirty.

### Nit

**4. The build's 10,006 comparisons are weaker than the number suggests** (`vendor-src/excalidraw/build.mjs:106-113`)

- 3,623 of the 10,006 calls throw in both versions, because half the random pairs have a > b. A shared throw counts
  as a match.
- Error messages are not compared.
- `n` never goes above 5, and custom digit sets are never tried.
- `"a0" + "0"` repeats `"a00"`.

The shim is still right: the 137,992 comparisons above found nothing. Smallest fix: order each pair, compare the
messages, and let `n` go up to about 50. `web/vendor/README.md:25` also says "10,000 inputs" where the build and the
report say 10,006 comparisons.

**5. Video still: no timeout, and the docs disagree on the time** (`web/app/board.js:398-417`, `docs/API.md:242`)

`videoFrame()` waits forever if neither `seeked` nor `error` fires, for example on a stalled read. `placeResult`
then leaves "Placing…" on screen. A video with no decodable frame also gives `videoWidth` 0, and a 1×1 "still"
counts as a real start picture.

The code seeks to `min(0.5 s, duration / 3)`. API.md says "a still of its first half second".

Smallest fix:

- Add a 10-second timeout that rejects (the plain-card fallback then takes over).
- Reject when `videoWidth` is 0.
- In API.md, say "a still from 0.5 s in (a third of the way into shorter clips)".

**6. The known gaps are stated only in the report and the vendor README** (`README.md:33`, `CHANGELOG.md:5-12`)

SVG export under the CSP is stated plainly, and correctly, in `web/vendor/README.md` and the report. I traced it:

- Excalidraw logs "Failed to use workers for subsetting", because its `bd()` check is false in a browser.
- `QB` then logs "Skipped glyph subsetting" and embeds each whole font as base64.

So "may embed whole fonts… and log a console error" is accurate, although it is two errors. "Video stills untested
on a real H.264 decoder" appears only in the report.

The README already lists the 3D viewer's CSP limit under Privacy, but says nothing for the board. Mermaid is absent
from the README too.

Smallest fix: one line in the README feature entry, and one in the CHANGELOG entry: "SVG export embeds whole fonts
(the page policy blocks Excalidraw's font subsetter); Mermaid is not included; video stills not yet checked in
desktop Chrome or Safari."

**7. `NOTICE` names ISC, but no listed package is ISC** (`NOTICE:11`)

`LICENSES.txt` has MIT, Apache-2.0 and 0BSD only. ISC enters only through the inlined `inherits` from finding 1.
This becomes true once finding 1 is fixed. Until then the line is wrong.

**8. API.md leaves out one load error** (`docs/API.md:228-230`)

`GET /api/boards/load` can also answer `500 read_failed` (`gateway/src/boards.rs:240`), which the list does not
mention. Smallest fix: add it.

**9. Excalidraw features that reach outside the studio stay on screen** (`web/app/board.js:92-100`)

The web-embed tool and pasted YouTube, Vimeo or X links make embeds that `frame-src 'none'` blocks. They show blank,
with a CSP console error. The library sidebar's "Browse libraries" opens libraries.excalidraw.com, and an "add to
Agave" round trip would then try a fetch that `connect-src 'self'` blocks.

Nothing happens without a click, so "nothing uses the network at runtime" holds. Smallest fix: pass
`validateEmbeddable={() => false}`, and optionally hide the embed tool and the library's browse link. Or say in the
docs that these do not work.

## The feature against the task

| Task | Code | Matches the report |
|---|---|---|
| Board rail entry | `#boardBtn` in the rail, `#board` hash, `showBoard()`/`leaveBoard()` in `app.js` | Yes |
| New, open, save, rename, delete | All five, with name rules, `rev` conflict handling and "Save over it", and two-click Delete that keeps the drawing | Yes |
| Unsaved changes kept when switching | Mode switch: yes, the editor stays mounted (checked: re-opening fetches only `/api/boards`). Opening another board: yes. New: **no** | No (finding 3) |
| Pictures shrunk | 1600 px long side, then 1200, 800 and 512 px steps; WebP 0.86 or JPEG; at most 2.5 M data-URL characters (about 1.9 MB) | Yes |
| Video and 3D as stills with a link back | Video: a frame at `min(0.5 s, duration / 3)`. 3D: Meshy's thumbnail output. Either falls back to a plain card. `link` is the result's `/outputs` file, opened in a new tab, with a grouped text label | Yes, with finding 5. Audio results also appear and go on as a plain card, which the report does not mention. |
| Frames become shots | Text with `frameId` is the prompt, in reading order, labels left out. The first image that is not a plain card is the start picture: the original `/outputs` file for a picture result, else the board copy through `/api/uploads` | Yes |
| Send to Video fills the form and runs nothing | Sets `state.refs`, the prompt and the mode, and calls only `renderTray`, `syncRefMode` and `autosize`. No `/api/generate` call is reachable | Yes |

## Size and load

Measured against the test-mode gateway, as the median of 6 fresh browser contexts. "Before" is `952d50e`'s
`web/app/` swapped into the same server.

| | Requests | Bytes | DOMContentLoaded | load |
|---|---|---|---|---|
| Studio before B1 | 24 | 225,352 | 123 ms | 197 ms |
| Studio with B1 | 24 | 228,253 | 138 ms | 226 ms |

The timing difference is within run-to-run noise on this machine. B1 adds 2.9 KB, the extra lines in `app.js`,
`app.css` and `index.html`. No request touches `/vendor/excalidraw/` or `/app/board.js` until Board is pressed.

- **First Board open:** 8 requests, 3,440,905 bytes in about 0.5 s:
  - `excalidraw.js` (3,180,454 bytes)
  - `excalidraw.css` (144,644 bytes)
  - `board.js`
  - 4 font files (85,888 bytes)
  - `/api/boards`
- **Fonts:** the other 13 MB of fonts load only when a board uses them.
- **Switching away and back:** only `/api/boards` is fetched.
- **Caching:** the guard gives vendor files `Cache-Control: no-cache`, so later page loads revalidate them with a 304.
- **Styles:** Excalidraw's stylesheet adds 16 `:root` custom properties (`--zIndex-*`, `--sa*`) and 24 class names
  outside `.excalidraw`. None collides with the studio's CSS.
- **Phone width:** at 390 px the page's scroll width is 390.

So yes: the 16.5 MB loads only when the board opens, and the rest of the studio is as fast as before.
