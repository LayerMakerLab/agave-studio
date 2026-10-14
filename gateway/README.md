# Agave Studio gateway

Rust (axum) backend on `127.0.0.1:4170`. Serves `../web` and the JSON API in `docs/API.md`.

## Run

```sh
cd gateway
cargo run --release                 # serves <repo>/web
cargo run --release -- --web /path  # serve a different web root
cargo test                          # unit and route tests (no network calls)
```

Open http://127.0.0.1:4170/ (home) and http://127.0.0.1:4170/app/ (workspace).

## Keys

- Slot names and docs links: `../config/keys.example.env`. Only these names are accepted.
- Put values in `../config/keys.env` (gitignored, written 0600), or export the same names as environment variables. Env vars win.
- Or POST from this machine: `POST /api/keys {"env":"FAL_KEY","value":"..."}`. Localhost only. Values are 8 to 512 characters with no whitespace.
- Keys are never returned or logged. `GET /api/keys` shows only whether each slot is set.
- A generate call with no key returns 412 `{"error":"missing_key","key":"FAL_KEY"}`.

## Other files

- `models.toml`: model registry. `verified = true` means the endpoint id was confirmed on the provider's own docs or
  API schema; it does not mean a live generation was run. Notes on each entry say what was checked and when.
- `engine/promptcraft/`: C++17 camera and style phrases, linked in through `build.rs`.
- `data/`: `jobs.json` (the gallery), `active-jobs.json` (jobs still running, with their provider handle so a restart
  picks them up; never keys), `studio.json` (Elements, project, edit timeline), `models/` (speech models) and
  `outputs/` (finished files, served at `/outputs/`). Created at startup, gitignored. A file that cannot be read is
  moved to `*.json.bad` and the gateway starts empty.

## Adapters

Implemented: fal (queue: Kling, Seedance, Wan, Hailuo, Veo, FLUX, Nano Banana, Seedream), Replicate, OpenAI images,
Gemini Veo, Gemini image, Black Forest Labs FLUX, ElevenLabs text to speech, Meshy image to 3D.
A model whose provider has no adapter returns 501 `adapter_not_implemented`.
