# Media credits for the homepage

Every picture and clip on the homepage (`web/index.html`) is listed here with the commit it comes from and what
made it. Nothing on the page comes from anywhere else.

## Sources

- **ef15fed** ("Bring back the Kling clips and the Library previews"): the stills were made with **Meshy**, and the
  clips were made from those stills with **Kling 3.0** (image to video) on fal. That commit's page footer says
  so: "Pictures on this page were made with Meshy and animated with Kling 3.0 on fal." The clip files carry no
  sound track.
- **dd2465f** (Agave Studio 0.1.0) has the same stills, byte for byte, and the play-leaf mark. Its page used
  them under the footer "Pictures on this page were made with Meshy."
- **Frames**: each poster is the first frame of its clip, and `lastframe-card-character.webp` is the last frame
  of its clip. They were taken from the ef15fed clips with ffmpeg and saved as WebP, with nothing added or
  changed.

## Clips (`web/assets/video`)

| File | Source | Made with | Where on the page |
|---|---|---|---|
| hero-atrium.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero strip, card 1 (The atrium) |
| hero-pitstop.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero strip, card 2 (Pit stop) |
| lb-samurai.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero strip, card 3 (Temple gate) |
| hero-dunes.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero strip, card 4 (Dust storm) |
| card-camera.mp4 | ef15fed | Kling 3.0 from a Meshy still | Tools, Video tile |
| card-image.mp4 | ef15fed | Kling 3.0 from a Meshy still | Reel |
| card-lipsync.mp4 | ef15fed | Kling 3.0 from a Meshy still | Reel |
| card-upscale.mp4 | ef15fed | Kling 3.0 from a Meshy still | Reel; Product pipeline, Video step (macro crop) |
| card-character.mp4 | ef15fed | Kling 3.0 from a Meshy still | Reel; Character pipeline, Clip 1 step |

## Stills (`web/assets/img`)

| File | Source | Made with | Where on the page |
|---|---|---|---|
| char-b.webp | ef15fed (same file at dd2465f) | Meshy | Reel; Character pipeline, Opening still step |
| char-c.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-perfume.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-ballet.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-moto.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-honey.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-noir.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-fisherman.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-silk.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| lb-astronaut.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| reel-t2v.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| tier-ultra.webp | ef15fed (same file at dd2465f) | Meshy | Reel |
| reel-upscale.webp | ef15fed (same file at dd2465f) | Meshy | Product pipeline, Hero still step |
| ref-1.webp | ef15fed (same file at dd2465f) | Meshy | Tools, Image tile (16:9 frame) |
| ref-3.webp | ef15fed (same file at dd2465f) | Meshy | Tools, Image tile (1:1 frame) |
| ref-4.webp | ef15fed (same file at dd2465f) | Meshy | Tools, Image tile (9:16 frame) |
| final-stage.webp | ef15fed (same file at dd2465f) | Meshy | Background of "Why it is free" |
| hero-atrium.jpg | ef15fed | First frame of hero-atrium.mp4 | Not shown; the website import uses it as the share picture (og:image) |

## Frames taken from the clips (`web/assets/img`)

| File | Taken from | Where on the page |
|---|---|---|
| poster-hero-atrium.webp | first frame of hero-atrium.mp4 | Hero card 1 poster |
| poster-hero-pitstop.webp | first frame of hero-pitstop.mp4 | Hero card 2 poster |
| poster-lb-samurai.webp | first frame of lb-samurai.mp4 | Hero card 3 poster |
| poster-hero-dunes.webp | first frame of hero-dunes.mp4 | Hero card 4 poster |
| poster-card-camera.webp | first frame of card-camera.mp4 | Video tile poster |
| poster-card-image.webp | first frame of card-image.mp4 | Reel poster |
| poster-card-lipsync.webp | first frame of card-lipsync.mp4 | Reel poster |
| poster-card-upscale.webp | first frame of card-upscale.mp4 | Reel and Product pipeline poster |
| poster-card-character.webp | first frame of card-character.mp4 | Reel and Character pipeline poster |
| lastframe-card-character.webp | last frame of card-character.mp4 | Character pipeline, Clip 2 step (the frame Clip 2 starts on) |

## Brand (`web/assets/brand`)

| File | Source | What it is |
|---|---|---|
| mark.svg | dd2465f | The play-leaf mark, drawn as SVG in this repo |
| icon.svg | dd2465f | The play-leaf mark on the app tile |
| apple-touch-icon.png | dd2465f | The play-leaf mark as a PNG icon |

## Notes

- The pipeline pictures are examples from the reel, not the output of a pipeline run, and the page says so.
- Pictures are cropped on the page (CSS `object-fit`); the files themselves are unchanged.
- The display face is Anybody (SIL Open Font License), loaded from Google Fonts. Geist and Geist Mono ship in
  `web/assets/fonts`.
