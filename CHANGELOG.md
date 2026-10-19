# Changelog

## Unreleased

- Kling 3.0, Kling O3 and Veo 3.1: Audio on or off (off is cheaper on fal), with the price to match.
- Kling 3.0 and Kling O3 take 3 to 15 seconds (were 5, 10 or 15, and 5 or 10).
- Kling 3.0 image to video takes a start picture and @Elements together: the Elements go to fal as `elements`.
- Dolly in on Kling 3.0 image to video moves the camera: the end picture is the start picture cropped in.
- A look on image to video goes on the start picture first (an image edit with its price shown), then you animate it.

- Pipelines in the Library: Product (photo, hero still, video, 3D model) and Character (sheet, Element, two clips that
  join on the last frame, edit). The full list price shows first, every paid step waits for your click, a failed step
  stops the run with Run again, and runs survive a reload or restart.
- Test mode (`AGAVE_FAKE_PROVIDERS=1`): every model is a local fake that makes TEST OUTPUT placeholders, for trying
  the studio without keys.
- The Edit timeline has a revision: a page holding an older copy can no longer save over newer clips.

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
