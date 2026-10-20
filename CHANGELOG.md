# Changelog

## Unreleased

- Spend log: every job that goes to a provider (the studio, the Playground, Compare, pipeline steps) adds one line to
  `data/spend.jsonl` when it ends, with the estimate its page showed before Run and the provider's list price for its
  options. Jobs carry the same as `cost`. Test mode writes its own log, marked test. See docs/API.md, "Spend log".
- A paid job or a key change needs the studio's page: every request that changes something needs a cookie made fresh
  at each start and given to a browser when it loads a page. A script or agent that only calls the API (a plain curl)
  is refused; a page opened before a restart, or a browser that keeps no cookies, shows "Reload the page". It is a
  speed bump, not a sign-in: a program that drives a browser gets the cookie too.
- Playground, from a first-time-user test: the code window names the run or form it is for, remembers its language
  and says how to install the client; each run in Your runs shows its words, and opening one brings its result into
  view (and focus, from the keyboard); Compare's picker marks models already in it; fal's help reads plainer ("The
  image to be used…", option labels for a choice's default, code shown as code); a template's note says what there is
  to change (words, fal's example file, your photo), and words typed into an empty prompt or over all of it while it
  loads are kept; a price per 1000 characters says "up to" (when no option can change the rate), in Compare too,
  since fal states no rule for shorter texts; a picture that
  can't be shown says "No preview"; the kinds of model wrap on wide screens and scroll with a fade on a phone; one run
  in Compare is announced as "The run is done".
- Playground Compare: one prompt (and one picture, for models that need one) to up to four fal models side by side.
  Each column shows the model's price and what it sends before Run all, and its result, time and price after; when a
  model has not been asked about yet, one price question lists every model and the total; Open takes a column to its
  own page with its settings. Run waits for files still uploading and adds a link typed but not added. Jobs now say
  when they ended (`finished_at`).
- Playground: fal's image, video, audio and 3D models, each with its own controls from fal's schema. Search and filter
  fal's catalog, fill the form (or edit the JSON), see fal's list price and average before Run, and keep every run with
  Tweak, Get code and Download. Opens with fal's example, so Run works at once. Test mode offers seven real models and
  fake results with no network; `AGAVE_TEST_PORT` runs it beside a real studio. Playground runs show in the gallery
  under their fal model's name.
- Veo 3.1 and Veo 3.1 Fast on your Gemini key are gone: Google shuts those preview models down on 22 October 2026.
  Veo 3.1 still runs on your fal key (fal lists those endpoints as deprecated on 17 November 2026). Old failed jobs
  on a model that is gone no longer offer Run again.
- Run again and Run failed again show their price before you press. Refused keys, missing credit and rate limits
  are explained in plain words.
- Captions with Whisper count as ready only when `whisper-cli` is on PATH; setup says what to install.
- Security: /outputs serves only names the engine writes; camera-move pictures are worked on in an owner-only
  folder, not the shared temp folder; pages may load scripts and connect only to the studio itself (so compressed
  Draco/KTX2 3D models do not display).
- Homepage: labels match the clips, lighter first load, no black flash on loops, keyboard fixes, unused files gone.

- Keyframed camera moves on Kling 3.0 image to video: Dolly in and Crash zoom send a crop of the start picture as
  the end picture; Dolly out and Orbit left/right make the end picture with an image edit, priced before Generate.
  The studio says which moves are keyframed and which go as words only. See docs/CAMERA.md.

- Kling 3.0, Kling O3 and Veo 3.1: Audio on or off (off is cheaper on fal), with the price to match.
- Kling 3.0 and Kling O3 take 3 to 15 seconds (were 5, 10 or 15, and 5 or 10).
- Kling 3.0 image to video takes a start picture and @Elements together: the Elements go to fal as `elements`
  (@Element1, @Element2...).
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
