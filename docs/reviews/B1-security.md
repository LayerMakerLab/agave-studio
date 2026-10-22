# B1 security and correctness review: Boards

Branch `feat/boards`, commit `7f52913`. Read only: no product code was changed. The probes below were scratch tests
and a scratch browser script, none of them committed.

## Checks

- `cargo test --offline` (in `gateway/`, after one `cargo fetch`): **174 passed, 0 failed, 2 ignored**.
- `cargo clippy --all-targets -- -D warnings`: **clean** (exit 0).
- `sha256sum -c web/vendor/excalidraw/SHA256SUMS`: all 236 files match.
- `gitleaks git . --redact` (8.30.1, as in CI): **1 leak**, `gcp-api-key` in `web/vendor/excalidraw/excalidraw.js:8`
  at `7f52913` (I0). CI's `secrets` job fails on it.

## Verdict: BLOCK

The routes are sound. Two findings break the repository's "0 leaks" rule:

- the bundle ships a Google API key, and CI's gitleaks `secrets` job fails on it (I0);
- opening a board file that someone else made can make the studio load files from any host (I1).

One correctness finding means some boards well under 25 MB cannot be saved (I2). All three are small fixes. The
Nits can wait.

## Findings

### I0 Important: the bundle ships a Google API key, and CI's `secrets` job fails on it

`web/vendor/excalidraw/excalidraw.js:8`, from `vendor-src/excalidraw/build.mjs`.

The build keeps Excalidraw's own `VITE_APP_FIREBASE_CONFIG`, which holds the `apiKey` of excalidraw.com's Firebase
project `excalidraw-room-persistence`. gitleaks 8.30.1 (the CI step) flags it with `gcp-api-key` at commit
`7f52913`, so the `secrets` job is red on any PR that carries `feat/boards`. Merged as is, it turns `main` red too.

This is upstream's public web key, not one of the user's keys, and the studio never calls Firebase:
`connect-src 'self'` blocks it. But the repository's rule is 0 leaks, its gate fails, and the key has no use here.

Smallest fix: in `build.mjs`, replace `VITE_APP_FIREBASE_CONFIG:'{...}'` with `'{}'`, the same way the font CDN
fallback is replaced. Fail the build if `apiKey` is still in the output, as the CDN check does. Then rebuild and
update `SHA256SUMS` and `web/vendor/README.md`. History keeps the key either way, so do it before `feat/boards`
merges, or squash the branch, so that `main` never contains it.

### I1 Important: a hostile board makes the page load files from any host

`web/app/board.js:124`, `:606`, `:618-619`.

Scenario: someone gets a `.excalidraw` file from someone else and puts it in `data/boards/`, or drops it straight onto
the canvas (Excalidraw's own drop handler loads it; `onDrop` only claims result drags). The file has:

- `files.f1.dataURL = "https://attacker.example/p.png"` on an image inside a frame;
- an `iframe` element with `customData.generationData = {status:"done", html:"<img src=https://attacker.example/i.png><link rel=stylesheet href=https://attacker.example/c.css>"}`.

What happened in the probe (headless Chromium, test-mode gateway, a local listener standing in for the attacker):
opening the board sent `GET /file.png` twice (Excalidraw's canvas, and the Shots list `<img src=s.file.dataURL>`),
`GET /iframe-img.png` and `GET /iframe.css`. The attacker learns the user's IP address and when the board was opened.
The CSP stopped the worst of it: the inline `<script>` in the srcdoc was refused (`script-src 'self'`), the meta
refresh and a YouTube embeddable were refused (`frame-src 'none'`), the srcdoc frame got no `allow-same-origin`, and
an SVG file's external `<image>` and `<script>` did nothing. Pressing Send to Video on that frame would load the
URL again through `loadImage` (`:619`) before the tainted canvas throws.

Cause: `X.restore` and `api.addFiles` keep any `dataURL` string and any `iframe` element, and the page CSP leaves
`img-src`/`style-src`/`font-src` open on purpose (reference pictures). That CSP is unchanged by this branch, which
is right. So the board has to filter what it loads.

Smallest fix: one `scrub(elements, files)` in `board.js`, run in `showScene` and in `changed()` (onChange covers
drop, paste and library imports). It should:

- keep only files whose `dataURL` matches `^data:image/(png|jpeg|webp|gif|svg\+xml);base64,`;
- mark `iframe` and `embeddable` elements `isDeleted` (or strip `customData.generationData`);
- pass `validateEmbeddable={() => false}` to `<Excalidraw>`.

In `renderShots` and `startPicture`, use `s.file.dataURL` only when it is a `data:` URL. Add a browser check that
opens a board like this one and asserts no request leaves 127.0.0.1.

### I2 Important: the server measures pretty-printed size, so some boards well under 25 MB cannot be saved

`gateway/src/boards.rs:157-158`, page check `web/app/board.js:231-232`.

Scenario: the page checks the board's compact JSON against 25 MB. The server re-serialises it with `to_vec_pretty`
and checks *that* against 25 MB. Indentation inflates nested arrays such as freedraw `points` several times. The
probe: a 14.3 MB compact body was refused with `413 board_too_large`. The user sees "This board is over 25 MB" on a
14 MB board and cannot save it. Unsaved work then lives only in the tab.

Smallest fix: `serde_json::to_vec(board)` (compact) at `boards.rs:157`. The file is still a valid `.excalidraw`.
Add a test that a body just under the limit with many small elements saves.

### N1 Nit: listing reads every file in full, with no size cap, while holding the lock

`gateway/src/boards.rs:118-122`, `:130-135` (also `:166` on save).

`info()` runs `fs::read` on the whole file to hash it, with no size check (`load` does have one). The probe put a
300 MB file in the folder: every `GET /api/boards` read it in full (7.7 s in a debug build) and listed it. While it
reads, the board mutex blocks every save, rename and delete. Fix: skip or cap files over `MAX_BOARD_BYTES` in
`info()`, as `load` does. The report's next step 5 (cache the rev by mtime and size) covers the rest.

### N2 Nit: `load` checks the path, then reads it again by name

`gateway/src/boards.rs:139-143`.

`symlink_metadata` checks the path; then `fs::read(&p)` opens it again by name and follows symlinks. A program
running as the user could swap in a symlink between the two calls. That needs local same-user access, and the target
must also parse as an Excalidraw scene, so the risk is low. Fix: open once with `O_NOFOLLOW | O_NONBLOCK` and check
`is_file()`, `nlink() == 1` and the length on that file handle's metadata, as `fsutil::append_private_line` does.

### N3 Nit: a stale `rev` is ignored once the board was renamed or deleted elsewhere

`gateway/src/boards.rs:162-174`.

Scenario: window A loads "Old" (rev r). Window B renames it to "New". Window A saves "Old" with `rev: r`, and
`same_name` finds nothing, so the save recreates "Old" (probe: 200). Nothing is lost, but the user gets two boards
and no warning. Fix: when `rev` is given and no board has the name, return `NotFound` (or `Changed("")`), so the page
can ask what to do.

### N4 Nit: an oversized rename or delete body gets a "board too large" error

`gateway/src/boards.rs:214-219`.

`rename` and `delete` keep axum's 2 MB default limit, but `body_fail` maps any 413 to
`{"error":"board_too_large","max_mb":25}`. Probe: a 3 MB rename body got exactly that. Fix: map 413 to
`bad_request` on those two routes, or give `body_fail` the route's own limit.

### N5 Nit: `openLink` opens any `https://` link

`web/app/board.js:108`.

The studio only ever sets `/outputs/<name>` links, but `openLink` also opens any `https://` link from a board in a
new tab. It opens with `noopener,noreferrer`, and the page sends `Referrer-Policy: no-referrer`. `javascript:` and
other schemes do nothing, and `script-src 'self'` would block them anyway. If links back to results must never leave
the host, drop the `https://` branch. Excalidraw's own link popup is an `<a href>`, so a middle-click there still
opens the link. That is the user's own click.

### N6 Nit: Enter in the name box skips the disabled Save button

`web/app/board.js:695` (button disabled at `:241`).

Pressing Enter twice quickly sends two saves with the same `rev`. The second gets `409 board_changed`, and the page
says the board "was saved from another window". Fix: return early in `save()` while `ui.save.disabled`.

### N7 Nit: "Browse libraries" sends the studio's local address to excalidraw.com

Vendored bundle, `library-menu-browse-button`.

The library sidebar's Browse libraries link opens
`https://libraries.excalidraw.com?...&referrer=http://127.0.0.1:4170/app/`. It only does that when the user clicks
it, but it is the one place where the local address goes into a URL off the host. Fix: hide
`.library-menu-browse-button` in `app.css`, or pass `libraryReturnUrl` with a neutral value.

## What was checked and holds

**Routes (`boards.rs`, `api.rs:213-217`).**

- All five routes sit inside the router that `guard()` wraps.
- Probes on rename and delete:
  - no launch cookie: `403 launch_token`;
  - a foreign `Origin`: `403 bad_origin`;
  - `Origin: null`: `403 bad_origin`.
- The existing tests cover the same for save, plus Host, `Sec-Fetch-Site` and a non-JSON type. The GET routes have
  no cookie check, like `/api/jobs`, which matches SECURITY.md.

**Names.**

- Allowed: ASCII letters, digits, space and `-_()` only, 1 to 60 bytes (bytes equal characters here), starting with a
  letter or digit, not ending in a space.
- Refused: `..`, `/`, `\`, NUL, a leading `.`, and Unicode (`Ünï`, a zero-width space, a combining accent), so
  macOS NFD folding cannot happen.
- A percent-encoded `%2E%2E` decodes to `..` and is refused.
- Letter-case collisions are refused on save and rename. A case-only rename is allowed.
- `CON`, `NUL` and `aux` are accepted, which is harmless on the supported platforms (macOS, Linux, WSL).

**25 MB limit.**

- `guard` runs first: no body is read without the cookie.
- Then `DefaultBodyLimit` makes the `Json` extractor stop reading at 25 MB + 64 KB, before it has the whole body.
  The existing test sends 25 MB + 1 KB and gets 413.
- Peak memory on a 25 MB body of tiny values is several times that once it is parsed into a `Value`. Only a holder
  of the launch cookie can send one.

**Writes.**

- `write_private`: `create_new`, mode 0600, `sync_all`, `rename` into a 0700 folder.
- The temp name `<name>.excalidraw.<hex>.tmp` can never be a board name (no dots), and `remove_leftovers` clears it
  at start.

**Links.**

- `file()` refuses symlinks and hard links (`nlink != 1`).
- Saving or renaming onto a planted link replaces the link and leaves its target untouched (probe: the target still
  read `keep`).

**Stale copies and races.**

- Every check-then-write runs inside `spawn_blocking` under one mutex, so concurrent requests cannot split a check
  from its write (N3 aside).

**A hostile file, server side.**

- `load` refuses a file over 25 MB, one that is not JSON, and JSON that is not a scene.
- It returns everything else as data, so the risk is on the page (I1).

**Page.**

- `board.js` builds every node with `createElement`, `createTextNode` and `textContent`. `setAttribute` gets fixed
  keys only.
- `board.js` has no `innerHTML`, `eval` or `Function`. `app.js`'s changes set text with `textContent` only.
- The page CSP is unchanged by this branch.
- `board.js` and the bundle load with same-origin `import()`. Fonts come from `/vendor/excalidraw/` through
  `EXCALIDRAW_ASSET_PATH`, and the bundle names no CDN.
- The bundle's absolute URLs are of three kinds:
  - links;
  - backends that `connect-src 'self'` blocks (json.excalidraw.com, the library backend, AI, collab);
  - embed templates that `frame-src`/`script-src` block.
- The bundle's `new Function` sits in the WASM glue, and its workers start from `data:` and `blob:` URLs. The CSP
  blocks all of them. This is already reported as the SVG-export limit.

**Dropped results.**

- A drop is looked up again by job id among the studio's own finished jobs, so another page's drag data cannot
  inject URLs.
- Each picture is re-encoded on a canvas to WebP or JPEG, capped at 1600 px and 2.5 M characters.
- Links and `customData.agave.picture` are `/outputs/<name>` paths, and the engine checks them again with
  `valid_output_name` (`/outputs/..` is refused).

**Send to Video.**

- `bridge.sendToVideo` sets the refs and prompt and switches mode. Nothing calls generate.
- Its only request is `POST /api/uploads` for a board picture, which saves a file and runs no job.

**Nothing leaked in the commit.**

- Apart from the upstream Firebase key in I0, the diff, the bundle, `web/vendor/README.md` and the lockfile have no
  key, no `data/` file, no `.env` and no local path. All 289 `resolved` entries are on registry.npmjs.org. My first
  grep missed I0; CI's gitleaks found it, and gitleaks 8.30.1 run here reports exactly that one finding.
- The names and e-mail addresses in the diff are upstream licence notices and the existing `LayerMaker LLC`
  copyright header.
- The commit author is `Agave Studio <noreply@agave.invalid>`.

## Not tested

- Desktop Chrome or Safari: everything above ran in headless Chromium.
- Whether Excalidraw asks before a dropped `.excalidraw` file replaces the board on screen.
- WSL on a `/mnt/c` checkout.
