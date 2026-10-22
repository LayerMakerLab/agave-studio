# Browser checks

Headless Chromium checks of the studio's Board view, run by hand against a test-mode gateway (fake providers, no
key, no network, nothing paid). They need Node 20 or newer and Playwright, and they make no request outside
127.0.0.1: each check fails if the page does.

```sh
cd gateway && AGAVE_FAKE_PROVIDERS=1 AGAVE_TEST_PORT=4171 cargo run --release &   # a test gateway on 4171
node tools/browser-checks/hostile-board.mjs
```

- `PLAYWRIGHT_DIR`: a `node_modules` folder that holds `playwright`, when it is not installed next to the checks.
- `CHROMIUM_PATH`: a Chromium to use instead of Playwright's own download.
- `AGAVE_BASE`: the gateway (default `http://127.0.0.1:4171`).

Each check deletes every board in the test gateway's `data/fake-providers/boards/` and prints `ok` or `FAIL` per
step, then exits non-zero on any failure, console error or warning, HTTP error, or request that left the studio.

| Check | What it does |
|---|---|
| `hostile-board.mjs` | Opens, drops and pastes a board whose picture is a web address and which carries an iframe and a web embed; presses Send to Video on its shot. Nothing may load from elsewhere. |
| `untitled-boards.mjs` | Draws on untitled boards and presses New, opens and deletes boards between them: every unsaved drawing stays in the Open list and comes back, and leaving the page still asks. |
