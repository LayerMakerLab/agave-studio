# Media credits for the homepage

Every picture and clip on the homepage (`web/index.html`) is listed here with where it comes from and what made
it. Nothing on the page comes from anywhere else.

## Sources

- **Made in Agave Studio, 2026-10-19.** New stills and clips were made through Agave's own API
  (`POST /api/generate`) on fal: stills with **Seedream 4** (`fal/seedream-4-t2i`) and **Seedream 4 Edit**
  (`fal/seedream-4-edit`), clips with **Kling 3.0 Pro** image to video (`fal/kling-3-pro-i2v`, 5 s) from those
  stills. Where a camera move or a look is named below, it was picked in Agave for that clip or still. The prompt
  for each one is kept with the job in the studio's library. The Dolly in, Looks and Elements hero clips were made
  after agave-studio PR #9, which sends Kling 3.0 an end picture for Dolly in, Elements beside a start picture, and a
  look on the start picture first.
- **ef15fed** ("Bring back the Kling clips and the Library previews"): stills made with **Meshy**, and clips made
  from those stills with **Kling 3.0** (image to video) on fal. That commit's page footer says so: "Pictures on
  this page were made with Meshy and animated with Kling 3.0 on fal."
- **dd2465f** (Agave Studio 0.1.0) has the same Meshy stills, byte for byte, and the play-leaf mark.
- **Encoding.** Clips were re-encoded with ffmpeg for the web (H.264, no sound, 1600, 720 or 480 px wide). Each poster
  is the first frame of its clip, and `lastframe-card-character.webp` is the last frame of its clip, saved as WebP
  with nothing added or changed. Stills were resized and saved as WebP.

## Hero cards (`web/assets/video`, posters in `web/assets/img`)

| File | Made with | In Agave | Card |
|---|---|---|---|
| hero-dolly.mp4 | Kling 3.0 Pro (audio off) from a Seedream 4 still of a dancer in a white hall | Camera: Dolly in (Agave sends the start picture and the same picture cropped in as the end picture) | Camera |
| hero-looks.mp4 | The pastel diner Seedream 4 still for 1 s, a wipe, then Kling 3.0 Pro (audio off) from that still after Agave's Film noir look edit (Seedream 4 Edit, `look` recipe). Joined with a wipe in ffmpeg; the first 0.2 s of the clip are trimmed | Look: Film noir; Camera: Static lock-off | Looks |
| hero-courier.mp4 | Kling 3.0 Pro from a Seedream 4 still (a courier in a neon alley) | Camera: Tracking side | Animate |
| hero-mara.mp4 | Kling 3.0 Pro (audio off) from a Seedream 4 still of an empty pier, with the Element @mara (saved from a Seedream 4 portrait) sent beside it as a Kling Element. The first 0.3 s are trimmed and the frame is cropped to its middle 70% in ffmpeg | Element @mara beside a start picture | Elements |
| img/ref-mara.webp | A face crop of that Seedream 4 portrait (the Element's picture) | Element @mara | Elements, the inset |

## Reel

| File | Made with | Source |
|---|---|---|
| video/reel-kayak.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Crane up) |
| video/reel-ink.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Static lock-off) |
| video/reel-skater.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Low-angle orbit) |
| video/reel-peonies.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Rack focus) |
| video/reel-surfer.mp4 | Kling 3.0 Pro (audio off) from a Seedream 4 still | Agave, 2026-10-19 (no camera move picked) |
| img/still-lighthouse.webp, still-matcha.webp, still-watchmaker.webp, still-crossing.webp, still-horse.webp, still-astronomer.webp, still-poster.webp, still-ceramics.webp | Seedream 4 (still-poster.webp shows text on purpose) | Agave, 2026-10-19 |
| video/card-image.mp4, card-lipsync.mp4, card-upscale.mp4, card-character.mp4 | Kling 3.0 from a Meshy still | ef15fed |
| img/char-b.webp, char-c.webp, reel-t2v.webp | Meshy | ef15fed (same files at dd2465f) |

## Tools

| File | Made with | Source | Where |
|---|---|---|---|
| img/tile-169.webp, tile-11.webp, tile-916.webp | Seedream 4 (small copies of the pool, hummingbird and tailor stills) | Agave, 2026-10-19 | Image tile |
| video/card-camera.mp4 | Kling 3.0 from a Meshy still | ef15fed | Video tile |

## Pipelines and "Why it is free"

| File | Made with | Source | Where |
|---|---|---|---|
| img/pipe-bowl.webp | Seedream 4 (the ramen bowl product photo) | Agave, 2026-10-19 | Product pipeline, the input photo |
| img/pipe-product-hero.webp | Nano Banana Edit: the Product pipeline's step 1 (Hero still) from pipe-bowl | Agave Library pipeline run, 2026-10-19 | Product pipeline, step 01 |
| video/pipe-product-video.mp4 | Kling 2.5 Turbo Pro, 5 s: the Product pipeline's step 2 (Video) from the hero still. The pipeline asks for a slow orbit; the camera held still and the light swept across instead | Agave Library pipeline run, 2026-10-19 | Product pipeline, step 02 |
| img/pipe-mara.webp | Seedream 4 (the @mara portrait) | Agave, 2026-10-19 | Character pipeline, the input photo |
| img/pipe-char-sheet.webp, pipe-char-still.webp | Nano Banana Edit: the Character pipeline's steps 1 (Character sheet) and 2 (Opening still, from the saved Element @hero) | Agave Library pipeline run, 2026-10-19 | Character pipeline, steps 01 and 02 |
| video/pipe-char-clip1.mp4, pipe-char-clip2.mp4 | Kling O3, 5 s each: the Character pipeline's Clip 1, and Clip 2 started on Clip 1's last frame | Agave Library pipeline run, 2026-10-19 | Character pipeline, steps 03 and 04 |
| img/final-stage.webp | Meshy | ef15fed (same file at dd2465f) | Background of "Why it is free" |
| img/hero-atrium.jpg | first frame of hero-atrium.mp4 | ef15fed | Not shown; the website import uses it as the share picture (og:image) |

## Brand (`web/assets/brand`)

| File | Source | What it is |
|---|---|---|
| mark.svg | dd2465f | The play-leaf mark, drawn as SVG in this repo |
| icon.svg | dd2465f | The play-leaf mark on the app tile |
| apple-touch-icon.png | dd2465f | The play-leaf mark as a PNG icon |

## Notes

- The pipeline lanes show one real run of each Library pipeline, made in Agave on 2026-10-19: every step's picture or
  clip is that step's own output, resized for the web. The Product run stopped before its 3D step (Meshy credits), and
  the page says so. The steps show each step's model and list price, as the studio shows them before the first step.
- Pictures are cropped on the page (CSS `object-fit`); the files themselves are only resized.
- The display face is Anybody (SIL Open Font License), loaded from Google Fonts. Geist and Geist Mono ship in
  `web/assets/fonts`.
