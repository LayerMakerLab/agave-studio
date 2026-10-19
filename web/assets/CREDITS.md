# Media credits for the homepage

Every picture and clip on the homepage (`web/index.html`) is listed here with where it comes from and what made
it. Nothing on the page comes from anywhere else.

## Sources

- **Made in Agave Studio, 2026-10-19.** New stills and clips were made through Agave's own API
  (`POST /api/generate`) on fal: stills with **Seedream 4** (`fal/seedream-4-t2i`) and **Seedream 4 Edit**
  (`fal/seedream-4-edit`), clips with **Kling 3.0 Pro** image to video (`fal/kling-3-pro-i2v`, 5 s) from those
  stills. Where a camera move or a look is named below, it was picked in Agave for that clip or still. The prompt
  for each one is kept with the job in the studio's library.
- **ef15fed** ("Bring back the Kling clips and the Library previews"): stills made with **Meshy**, and clips made
  from those stills with **Kling 3.0** (image to video) on fal. That commit's page footer says so: "Pictures on
  this page were made with Meshy and animated with Kling 3.0 on fal."
- **dd2465f** (Agave Studio 0.1.0) has the same Meshy stills, byte for byte, and the play-leaf mark.
- **Encoding.** Clips were re-encoded with ffmpeg for the web (H.264, no sound, 1600 or 720 px wide). Each poster
  is the first frame of its clip, and `lastframe-card-character.webp` is the last frame of its clip, saved as WebP
  with nothing added or changed. Stills were resized and saved as WebP.

## Hero cards (`web/assets/video`, posters in `web/assets/img`)

| File | Made with | In Agave | Card |
|---|---|---|---|
| hero-courier.mp4 | Kling 3.0 Pro from a Seedream 4 still (a courier in a neon alley) | Camera: Tracking side | Camera |
| hero-looks.mp4 | Four Seedream 4 stills, same prompt and seed: no look, 35mm film, Film noir, VHS. Cut together with wipes and a slow push in ffmpeg | Looks | Looks |
| hero-diner.mp4 | Kling 3.0 Pro from a Seedream 4 still (a pastel diner) | Camera: Push through | Animate |
| hero-mara.mp4 | Kling 3.0 Pro from a Seedream 4 Edit still of the Element @mara, which was saved from a Seedream 4 portrait | Element @mara; Camera: Handheld | Elements |
| img/ref-mara.webp | A face crop of that Seedream 4 portrait (the Element's picture) | Element @mara | Elements, the inset |

## Reel

| File | Made with | Source |
|---|---|---|
| video/reel-kayak.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Crane up) |
| video/reel-ink.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Static lock-off) |
| video/reel-skater.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Low-angle orbit) |
| video/reel-peonies.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Camera: Rack focus) |
| video/hero-dancer.mp4 | Kling 3.0 Pro from a Seedream 4 still | Agave, 2026-10-19 (Dolly in was picked, but the camera did not move, so the page names no move) |
| img/still-lighthouse.webp, still-matcha.webp, still-watchmaker.webp, still-crossing.webp, still-horse.webp, still-astronomer.webp, still-poster.webp, still-ceramics.webp | Seedream 4 (still-poster.webp shows text on purpose) | Agave, 2026-10-19 |
| video/card-image.mp4, card-lipsync.mp4, card-upscale.mp4, card-character.mp4 | Kling 3.0 from a Meshy still | ef15fed |
| img/char-b.webp, char-c.webp, reel-t2v.webp | Meshy | ef15fed (same files at dd2465f) |

## Tools

| File | Made with | Source | Where |
|---|---|---|---|
| img/tile-169.webp, tile-11.webp, tile-916.webp | Seedream 4 (small copies of the pool, ramen bowl and tailor stills) | Agave, 2026-10-19 | Image tile |
| video/card-camera.mp4 | Kling 3.0 from a Meshy still | ef15fed | Video tile |

## Pipelines and "Why it is free"

| File | Made with | Source | Where |
|---|---|---|---|
| img/pipe-bowl.webp | Seedream 4 (the ramen bowl product photo) | Agave, 2026-10-19 | Product pipeline, the input photo |
| img/pipe-mara.webp | Seedream 4 (the @mara portrait) | Agave, 2026-10-19 | Character pipeline, the input photo |
| img/final-stage.webp | Meshy | ef15fed (same file at dd2465f) | Background of "Why it is free" |
| img/hero-atrium.jpg | first frame of hero-atrium.mp4 | ef15fed | Not shown; the website import uses it as the share picture (og:image) |

## Brand (`web/assets/brand`)

| File | Source | What it is |
|---|---|---|
| mark.svg | dd2465f | The play-leaf mark, drawn as SVG in this repo |
| icon.svg | dd2465f | The play-leaf mark on the app tile |
| apple-touch-icon.png | dd2465f | The play-leaf mark as a PNG icon |

## Notes

- The pipeline photos are example inputs, not the output of a pipeline run, and the page says so. The steps show
  each step's model and list price, as the studio shows them before the first step.
- Pictures are cropped on the page (CSS `object-fit`); the files themselves are only resized.
- The display face is Anybody (SIL Open Font License), loaded from Google Fonts. Geist and Geist Mono ship in
  `web/assets/fonts`.
