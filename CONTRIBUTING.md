# Contributing

Thanks for helping. Agave Studio is small on purpose, so keep changes focused. Everyone taking part follows the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Before you open a pull request

- `cd gateway && cargo test --release` passes. Tests must not call the network.
- `cargo clippy --all-targets` shows no warnings.
- Web changes work in the studio (`/app/`) at desktop and phone width, with no console errors.
- New models go in `gateway/models.toml`. Mark a model `verified = true` only once its endpoint id and request fields
  are confirmed on the provider's own docs or API schema, and note the source and date beside it.
- Never commit keys. `config/keys.env` and `data/` are gitignored.

## Ground rules

- The gateway listens on 127.0.0.1 only and has no sign-in. Changes must keep the `guard()` checks
  (Host, Origin, Sec-Fetch-Site and JSON content type) on every route.
- The web app uses `textContent`, never `innerHTML`, for anything a user or a provider can influence.
- Run ffmpeg and the native tools with argument arrays, never through a shell.

By contributing you agree that your work is licensed under the Apache License 2.0.
