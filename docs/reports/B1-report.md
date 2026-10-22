# B1: Boards in Agave — report

Branch `feat/boards`. The Board view at `/app/#board` (a Board entry in the rail) runs the real Excalidraw editor
from a local bundle, saves boards on the engine, takes finished results by drag and drop, and turns frames into shots
for the Video form.

## What works

- **Local Excalidraw.** Excalidraw 0.18.1 with React 18.3.1 and ReactDOM 18.3.1 is built with esbuild 0.28.2 into one
  ES module, `web/vendor/excalidraw/excalidraw.js`. Its stylesheet and every font it loads sit beside it. The page
  points `EXCALIDRAW_ASSET_PATH` at `/vendor/excalidraw/`, and the build also points Excalidraw's own CDN fallback
  there, then fails if the bundle still names a CDN. `vendor-src/excalidraw/build.sh` (`npm ci --ignore-scripts`,
  then `build.mjs`) rebuilds the folder byte for byte; two builds gave the same `SHA256SUMS`.
- **Licences.** The build reads the licence of every package esbuild pulled in and stops on anything other than MIT,
  Apache-2.0, BSD or ISC. All 82 bundled packages pass. Fonts are OFL-1.1, except Comic Shanns, which is MIT. Every
  notice is in `web/vendor/excalidraw/LICENSES.txt` and summarised in `NOTICE`. `web/vendor/README.md` gives source,
  version, licence and the sha256 of each of the 236 files.
- **Gateway** (`gateway/src/boards.rs`): `GET /api/boards`, `GET /api/boards/load?name=`, `POST /api/boards/save`,
  `/rename` and `/delete`. Boards are saved as `data/boards/<name>.excalidraw`. All five routes sit behind the
  existing `guard()` (Host, Origin, Sec-Fetch-Site, JSON, launch cookie).
  - Names are a strict file-name set with no dots or slashes, and names that differ only in letter case count as the
    same board.
  - Each board is limited to 25 MB, and the save route's body limit matches.
  - Writes are atomic and owner-only, through `fsutil::write_private`.
  - Symlinks and hard links are never treated as boards, and saving over a planted link replaces the link.
  - A save can carry the `rev` it loaded; if the board changed since, it gets 409 `board_changed` and the page offers
    "Save over it".
  - Leftover temp files in `data/boards` are cleared at startup.
- **Studio.** The board bar has Open, a name box, and New, Save, Rename and Delete. Delete takes two clicks, and the
  drawing stays on screen so a mistaken delete can be saved again. Unsaved changes survive switching to another mode
  and back, because the editor stays mounted. They also survive opening another board or a new one: they are kept in
  memory, marked "(unsaved changes)" in the list, and come back when reopened. The browser asks before leaving the
  page with unsaved changes.
- **Drop.** The Results column holds the studio's last 24 finished results as cards. Drag one onto the board, or press
  Place.
  - Pictures are shrunk to 1600 px on the long side and about 1.9 MB at most (WebP, or JPEG as a fallback), and get
    smaller steps if needed.
  - Videos go on as a still from about 0.5 s in. 3D models go on as their thumbnail, or a plain card if they have none.
    Both link to the result's file, which opens in a new tab, and get a text label grouped with the still.
  - A result dropped on a frame joins that frame.
- **Frames become shots.** Each frame lists as a shot, in reading order. Its text (labels left out) is the prompt. Its
  first picture is the start image: the result's original `/outputs` file, or for other pictures the board's copy,
  uploaded with `/api/uploads`. Plain cards are never used. Send to Video switches to Video, fills the prompt and the
  reference tray, and says to check the model and cost and press Generate. It starts no job: the check counted 42 jobs
  before and after.

## Checks

- `cargo test --offline`: **174 passed**, 0 failed, 2 ignored. Before this branch it was 168; the 6 new tests are in
  `boards.rs`.
- `cargo clippy --all-targets -- -D warnings`: clean.
- Headless Chromium against a test-mode gateway (`AGAVE_FAKE_PROVIDERS=1`):
  - At 1440×900 the check drew a frame, wrote text in it, dropped a picture, placed a video, then saved, reopened,
    switched away and back, sent to Video, renamed and deleted.
  - At 390×844 it opened the saved board. The page scroll width matched the viewport width (390), so there was no
    sideways scroll.
  - Both: **no console errors or warnings, no request off 127.0.0.1**.

## Bundle size

- `excalidraw.js`: 3,180,454 bytes. About 1.8 MB of that is Excalidraw's own font-subsetting WebAssembly, stored as
  text.
- `excalidraw.css`: 144,644 bytes.
- `fonts/`: 13 MB, mostly the Xiaolai CJK font in 209 pieces.
- In total, 236 files and 16.5 MB. The bundle loads only when the board first opens, and fonts load only when a board
  uses them.

## What does not work, and decisions to check

- **Left out for licence reasons:**
  - Mermaid ("Mermaid to Excalidraw" now says it is not available).
  - Liberation Sans 1.05, which is GPL with a font exception; Excalidraw only uses it server-side.
  - `pako`, which is MIT and Zlib; replaced by a shim on `fflate` (MIT).
- **Licence decision for you:** `fractional-indexing` is CC0-1.0, which is outside the allowed list, so I replaced it
  with my own Apache-2.0 version of the same scheme. The build compares it with the original on 10,006 inputs before
  writing. CC0 is public domain, so if you count it as acceptable, drop the alias in `build.mjs`.
- **SVG export:** Excalidraw subsets fonts with WebAssembly, and the page policy (`script-src 'self'`, no
  `wasm-unsafe-eval`) blocks that. An SVG export may embed whole fonts or fail, and log a console error. PNG export
  is unaffected. I did not change the page policy.
- **Video stills in this test browser:** headless Chromium here has no H.264 decoder, so video thumbnails in Results
  were blank and the placed "still" fell back to a plain card. That fallback path is tested. Chrome and Safari decode
  H.264, but real video stills were not seen in this run.
- **First key after load:** right after the page loads, Excalidraw ignores the first keyboard shortcut until it has
  focus. This is Excalidraw's behaviour; the toolbar works at once.
- **Drag sources:** the drag source is the board's Results column. Gallery cards are hidden while the board shows,
  so they are not drag sources.
- **Listing cost:** each list call reads every board file to hash it. This is fine for tens of boards, but slow with
  hundreds of large ones.
- **Video form limits:** Send to Video fills one shot at a time. A shot whose words are over the engine's 2,000
  characters is refused at Generate with the usual message.
- **Network at build time:** `cargo fetch` had to run once, because this container had no crate cache. Building the
  bundle needs npm. Nothing uses the network at runtime.

## Next steps

1. Check video stills and the drag feel in desktop Chrome and Safari with real H.264 results.
2. Decide on CC0 (`fractional-indexing`) and on allowing `wasm-unsafe-eval` for SVG export.
3. "Send all shots": queue each frame as its own filled Video form, each still waiting for its own Generate.
4. Let gallery cards drag onto the board when both are visible (a split view).
5. Cache `rev` by mtime and size so listing does not hash every board.
