# Changelog

## Unreleased

- Pipelines in the Library: Product (photo, hero still, video, 3D model), Character (sheet, Element, two clips that
  join on the last frame, edit) and a short film on your own ComfyUI (your photo as an Element, two LTX-Video shots
  joined on the last frame, edit). The full list price shows first, every paid step waits for your click, a failed step stops the run
  with Run again, and runs survive a reload or restart.
- Your own ComfyUI as a provider, through comfy-api-proxy and the Comfy API v2 (`COMFY_BASE_URL`, no key, nothing
  billed): LTX-Video 0.9.5 and FLUX.1 schnell.
- `COMFY_CLIENT=sdk` runs ComfyUI jobs through the official Comfy SDK (comfy-sdk 0.5.0, `tools/comfy-sdk/agave_comfy.py`,
  started with uv); `builtin`, the studio's own client, stays the default. Both share the same checks and caps.
- LTX-Video 2B 0.9.8 distilled on your own ComfyUI (stock nodes, first frame kept); the ComfyUI short film pipeline
  uses it. Ran live through both clients. Its second shot gets the Element photo as its last-frame keyframe
  (LTXVAddGuide, frame_idx -1, strength 1.0): the shot ends back on the Element's look and close to its pose. It is not
  a general look keeper; a brief mid-shot wobble is possible (tested live with 3 seeds).
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
