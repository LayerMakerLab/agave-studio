# B1: Boards in Agave — report

Branch `feat/boards`. The Board view at `/app/#board` (a Board entry in the rail) runs the real Excalidraw editor
from a local bundle, saves boards on the engine, takes finished results by drag and drop, and turns frames into shots
for the Video form.

## What works

- **Local Excalidraw.** Excalidraw 0.18.1 with React 18.3.1 and ReactDOM 18.3.1 is built with esbuild 0.28.2 into one
  ES module, `web/vendor/excalidraw/excalidraw.js`. Its stylesheet and every font it loads sit beside it. The page
  points `EXCALIDRAW_ASSET_PATH` at `/vendor/excalidraw/`, and the build also points Excalidraw's own CDN fallback
  there, then fails if the bundle still names a CDN or holds an API key. `vendor-src/excalidraw/build.sh`
  (`npm ci --ignore-scripts`, then `build.mjs`) rebuilds the folder byte for byte; two builds gave the same
  `SHA256SUMS`.
- **Licences.** The build reads the licence of every package esbuild pulled in and stops on anything other than MIT,
  Apache-2.0, BSD, ISC, 0BSD or CC0-1.0. All 83 bundled packages pass. It also finds code inlined in other packages'
  prebuilt files (7 packages) and stops on any it has no notice for. Fonts are OFL-1.1, except Comic Shanns, which
  is MIT. Every notice is in `web/vendor/excalidraw/LICENSES.txt` and summarised in `NOTICE`.
  `web/vendor/README.md` gives source, version, licence and the sha256 of each of the 236 files.
- **Gateway** (`gateway/src/boards.rs`): `GET /api/boards`, `GET /api/boards/load?name=`, `POST /api/boards/save`,
  `/rename` and `/delete`. Boards are saved as `data/boards/<name>.excalidraw`. All five routes sit behind the
  existing `guard()` (Host, Origin, Sec-Fetch-Site, JSON, launch cookie).
  - Names are a strict file-name set with no dots or slashes, and names that differ only in letter case count as the
    same board. Every lookup matches the exact name in the folder's listing, so it behaves the same on macOS.
  - Each board is limited to 25 MB of compact JSON (as the page measures it), and the save route's body limit
    matches. Larger files in the folder are neither read nor listed.
  - Each file is opened once, without following links, and checked on that handle.
  - Writes are atomic and owner-only, through `fsutil::write_private`.
  - Symlinks and hard links are never treated as boards, and saving over a planted link replaces the link.
  - A save can carry the `rev` it loaded. If the board changed since, it gets 409 `board_changed`, and the page offers
    "Save over it". If the board was renamed or deleted since, it gets 404, and the page offers to save it again.
  - Leftover temp files in `data/boards` are cleared at startup.
- **Studio.** The board bar has Open, a name box, and New, Save, Rename and Delete. Delete takes two clicks, and the
  drawing stays on screen, as an untitled board of its own, so a mistaken delete can be saved again. Unsaved changes
  survive switching to another mode and back, because the editor stays mounted. They also survive opening another
  board or a new one: each board, and each untitled board, is kept in memory under its own entry. Saved boards show
  "(unsaved changes)" in the Open list, untitled ones "Untitled board N (unsaved)", and both come back when reopened.
  The browser asks before leaving the page with unsaved changes.
- **Boards from elsewhere.** A board only ever shows embedded pictures. A picture whose file is a web address, and
  every iframe or web embed, is left out when a board opens, is dropped on the canvas or changes, and the page says
  so. A paste carrying web pictures or embeds is refused. The web-embed tool and the library's Browse libraries link
  are hidden, and a picture's link opens only when it is one of the studio's own `/outputs/` files.
- **Drop.** The Results column holds the studio's last 24 finished results as cards. Drag one onto the board, or press
  Place.
  - Pictures are shrunk to 1600 px on the long side and about 1.9 MB at most (WebP, or JPEG as a fallback), and get
    smaller steps if needed.
  - Videos go on as a still from 0.5 s in (a third of the way into shorter clips). If no frame can be read within
    10 s, they go on as a plain card instead. 3D models go on as their thumbnail, or a plain card if they have none.
    Both link to the result's file, which opens in a new tab, and get a text label grouped with the still.
  - A result dropped on a frame joins that frame.
- **Frames become shots.** Each frame lists as a shot, in reading order. Its text (labels left out) is the prompt. Its
  first picture is the start image: the result's original `/outputs` file, or for other pictures the board's copy,
  uploaded with `/api/uploads`. Plain cards are never used. Send to Video switches to Video, fills the prompt and the
  reference tray, and says to check the model and cost and press Generate. It starts no job: the check compares the
  job count before and after.

## Checks

After the review fixes:

- `cargo test --offline`: **177 passed**, 0 failed, 2 ignored. Before this branch it was 168. The 9 new tests are in
  `boards.rs`: 6 from the first round and 3 from the review fixes. The review fixes also extended 3 existing tests.
- `cargo clippy --all-targets -- -D warnings`: clean.
- gitleaks 8.30.1, the CI `secrets` step (`gitleaks git . --redact --exit-code 1`): no leaks over the full history.
- `sha256sum -c web/vendor/excalidraw/SHA256SUMS`: all 236 files match. The README list matches `SHA256SUMS`.
- Browser checks, now in `tools/browser-checks/`. Each ran in headless Chromium against a test-mode gateway
  (`AGAVE_FAKE_PROVIDERS=1`) and printed `ok` on every step:
  - `board.mjs`, at 1440×900 and 390×844: the whole flow, with no sideways scroll at 390 px. It passed 5 runs. One
    earlier run printed a problem line that my output filter hid, so I don't know what it was. It did not come back
    in the next 4 runs.
  - `hostile-board.mjs`: the review's hostile board, opened, dropped as a file, pasted, and Send to Video on its shot.
  - `untitled-boards.mjs`: New, reopening and Delete with several untitled boards.
  - All three: **no console errors or warnings, no request off 127.0.0.1**.
  - Against the code from before the fixes, `hostile-board.mjs` failed 8 of its steps, with requests to
    attacker.example. `untitled-boards.mjs` failed on New.

## Bundle size

- `excalidraw.js`: 3,180,022 bytes. About 1.8 MB of that is Excalidraw's own font-subsetting WebAssembly, stored as
  text.
- `excalidraw.css`: 144,644 bytes.
- `LICENSES.txt`: 145,493 bytes.
- `fonts/`: 13 MB, mostly the Xiaolai CJK font in 209 pieces.
- In total, 236 files and 16.5 MB (16,538,212 bytes). The bundle loads only when the board first opens, and fonts load
  only when a board uses them.

## What does not work, and decisions to check

- **Left out for licence reasons:**
  - Mermaid ("Mermaid to Excalidraw" now says it is not available).
  - Liberation Sans 1.05, which is GPL with a font exception; Excalidraw only uses it server-side.
  - `pako`, which is MIT and Zlib; replaced by a shim on `fflate` (MIT).
- **fractional-indexing:** the original package (CC0-1.0, by Rocicorp) ships as published, with its CC0 dedication in
  `LICENSES.txt`. The earlier shim and its comparison are gone.
- **SVG export:** Excalidraw subsets fonts with WebAssembly, and the page policy (`script-src 'self'`, no
  `wasm-unsafe-eval`) blocks that. An SVG export embeds each whole font and logs two console errors ("Failed to use
  workers for subsetting" and "Skipped glyph subsetting"). PNG export is unaffected. I did not change the page
  policy. The README and CHANGELOG now state this.
- **Video stills in this test browser:** headless Chromium here has no H.264 decoder, so video thumbnails in Results
  were blank and the placed "still" fell back to a plain card. That fallback path is tested. Chrome and Safari decode
  H.264, but real video stills were not seen in this run.
- **First key after load:** right after the page loads, Excalidraw ignores the first keyboard shortcut until it has
  focus. This is Excalidraw's behaviour; the toolbar works at once.
- **Drag sources:** the drag source is the board's Results column. Gallery cards are hidden while the board shows,
  so they are not drag sources.
- **Listing cost:** each list call reads every board file of 25 MB or less to hash it. This is fine for tens of
  boards, but slow with hundreds of large ones.
- **macOS:** the case fix is checked on Linux only. No case-insensitive filesystem could be mounted here.
- **Secret in history:** the first board commit (`7f52913`) still holds Excalidraw's public Firebase web key.
  `.gitleaksignore` names that one finding, so CI passes. Removing it from history would mean squashing the branch
  and force-pushing, which I have not done.
- **Video form limits:** Send to Video fills one shot at a time. A shot whose words are over the engine's 2,000
  characters is refused at Generate with the usual message.
- **Network at build time:** `cargo fetch` had to run once, because this container had no crate cache. Building the
  bundle needs npm. Nothing uses the network at runtime.

## Next steps

1. Check video stills and the drag feel in desktop Chrome and Safari with real H.264 results.
2. Decide on allowing `wasm-unsafe-eval` for SVG export, and on squashing the branch before it merges, so `main`
   never holds the Firebase key (then drop `.gitleaksignore`).
3. "Send all shots": queue each frame as its own filled Video form, each still waiting for its own Generate.
4. Let gallery cards drag onto the board when both are visible (a split view).
5. Cache `rev` by mtime and size so listing does not hash every board.
6. Run `cargo test` on macOS to confirm the letter-case fix there.

## Review fixes

Two reviews came in: security (`docs/reviews/B1-security.md` on `claude/b1-review-security`) and fit
(`docs/reviews/B1-fit.md` on `claude/b1-review-fit-wh9dxc`). There is one commit per finding.

| Finding | Fix | Commit |
|---|---|---|
| macOS bug | `load("Shots")` found SHOTS.excalidraw after a rename on a case-insensitive disk. Every lookup now uses the exact name from the folder's listing. | `f96da43` |
| Security I2 | The 25 MB limit is measured on compact JSON. New test: a body just under the limit, made of many small strokes. | `f233bd4` |
| Security N1 | Files over 25 MB in the folder are not read or listed. | `8e5fff5` |
| Security N2 | One handle (`O_NOFOLLOW`, `O_NONBLOCK`); type, link count and length are checked on it. | `214ba39` |
| Security N3 | A save with a `rev` for a board that no longer exists gets 404, and the page asks. | `3614a79` |
| Security N4 | An oversized rename or delete body gets `bad_request`; the API.md part is in `1e659e9`. | `e7473f5`, `1e659e9` |
| Security I1 | `scrub()` runs when a board opens and on every change, and on board files dropped on the canvas. Pastes that reach the web are refused, `validateEmbeddable` returns false, and Shots and Send to Video use `data:` pictures only. Covered by `hostile-board.mjs`. | `9a3d196` |
| Security N5 | `openLink` opens only `/outputs/` links. | `cd1239f` |
| Security N6 | `save()` returns early while Save is disabled. | `859184b` |
| Security N7 | Browse libraries is hidden. | `c191c33` |
| Security I0, fit 10 | The build blanks Excalidraw's Firebase config and fails on any API key. `.gitleaksignore` covers the old commit. | `efe095b` |
| Fit 1 | Notices for fonteditor-core, harfbuzzjs (with its Apache-2.0 Zephyr and MIT/NCSA emmalloc parts), glur, multimath, object-assign, webworkify and inherits. The build fails on inlined code with no notice. | `a5e2640` |
| Fit 2, 4 | The original fractional-indexing package, credited as published. The comparison is dropped with the shim. | `57c6d8c` |
| Fit 3 | Each untitled board is kept under its own key. New and Delete keep every unsaved drawing. Covered by `untitled-boards.mjs`. | `9248352` |
| Fit 5 | Video stills time out after 10 s and are refused when the video is zero-width. API.md now says when the still is taken. | `ce73ccf` |
| Fit 9 | The web-embed tool is hidden, and API.md says what a board leaves out. | `c99d451` |
| Fit 6 | The known gaps are listed in the README and the CHANGELOG. | `5507082` |
| Fit 7 | NOTICE names exactly the licences in `LICENSES.txt`. | `fbbf643` |
| Fit 8 | API.md lists `500 read_failed`. | `ab6e68c` |
| (checks) | The end-to-end board check is kept in `tools/browser-checks/board.mjs`. | `e2674bf` |

The bundle was rebuilt three times: for I0, fit 1 and fit 2. Each rebuild updated `SHA256SUMS` and the list in
`web/vendor/README.md`. The last rebuild reproduced byte for byte.
