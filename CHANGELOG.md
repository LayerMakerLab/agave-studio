# Changelog

## 0.1.0 (2026-10-13)

First public release.

- Generate: text to image, image to image, text to video, image to video, text to speech and image to 3D from one
  queue, on your own provider keys (fal.ai, Replicate, Google Gemini, OpenAI, Black Forest Labs, ElevenLabs, Meshy).
- Direct: 20 camera moves, 10 looks, a prompt library, and Elements called with `@tag`.
- Edit: trim, split and join clips, auto captions (on-device), remove pauses, reframe to 9:16, clean up voices,
  loudness to -14 LUFS, export for YouTube or Shorts.
- Jobs that are still running when the studio closes are picked up again on the next start.
- Optional macOS app and on-device tools (Parakeet captions, Follow reframe).
- Security: the API refuses requests from other websites, pages send a stricter CSP and no referrer, error
  messages never carry keys or local paths, and `data/` is owner-only on disk.
- The fonts ship with the studio, so opening it no longer loads anything from Google Fonts.
