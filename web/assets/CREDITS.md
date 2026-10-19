# Media credits for the homepage

Every picture and clip on the homepage (`web/index.html`) is listed here with the commit it comes from and what
made it. Nothing on the page comes from anywhere else.

Sources:

- **ef15fed** ("Bring back the Kling clips and the Library previews"): the stills were made with **Meshy** and the
  clips were made from those stills with **Kling 3.0** (image to video) on fal. The clip files carry no sound track. That commit's page
  footer says so: "Pictures on this page were made with Meshy and animated with Kling 3.0 on fal."
- **dd2465f** (Agave Studio 0.1.0): the same Meshy stills (identical files) and the play-leaf mark.
- **Frames**: a poster is the first frame of its clip, and `lastframe-card-character.webp` is the last frame of
  its clip. They were taken from the ef15fed clips with ffmpeg and saved as WebP. Nothing was added or changed.

## Clips (`web/assets/video`)

| File | Source | Made with | Where on the page |
|---|---|---|---|
| hero-pitstop.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero card 01, Video |
| hero-atrium.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero card 02, Camera |
| hero-dunes.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero card 03, Image to video |
| hero-soundstage.mp4 | ef15fed | Kling 3.0 from a Meshy still | Hero card 04, Edit |
| card-camera.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel |
| card-image.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel |
| lb-samurai.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel |
| card-lipsync.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel |
| card-upscale.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel; Product pipeline, Video step |
| card-character.mp4 | ef15fed | Kling 3.0 from a Meshy still | The reel; Character pipeline, Clip 1 step |

## Stills (`web/assets/img`)

| File | Source | Made with | Where on the page |
|---|---|---|---|
| lb-perfume.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-ballet.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-pitstop.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-moto.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-honey.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-noir.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-fisherman.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-silk.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| lb-astronaut.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| reel-t2v.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| camera-plate.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| char-a.webp | ef15fed (same file at dd2465f) | Meshy | Character pipeline, Character sheet step |
| char-b.webp | ef15fed (same file at dd2465f) | Meshy | The reel; Character pipeline, Opening still step |
| char-c.webp | ef15fed (same file at dd2465f) | Meshy | The reel |
| reel-upscale.webp | ef15fed (same file at dd2465f) | Meshy | Product pipeline, Hero still step |
| final-stage.webp | ef15fed (same file at dd2465f) | Meshy | Background of "Why it is free" |
| hero-atrium.jpg | ef15fed | First frame of hero-atrium.mp4 | Not shown; the website import uses it as the share picture (og:image) |

## Frames taken from the clips (`web/assets/img`)

| File | Taken from | Where on the page |
|---|---|---|
| poster-hero-pitstop.webp | first frame of hero-pitstop.mp4 | Hero card 01 poster |
| poster-hero-atrium.webp | first frame of hero-atrium.mp4 | Hero card 02 poster |
| poster-hero-dunes.webp | first frame of hero-dunes.mp4 | Hero card 03 poster |
| poster-hero-soundstage.webp | first frame of hero-soundstage.mp4 | Hero card 04 poster |
| poster-card-camera.webp | first frame of card-camera.mp4 | Reel poster |
| poster-card-image.webp | first frame of card-image.mp4 | Reel poster |
| poster-lb-samurai.webp | first frame of lb-samurai.mp4 | Reel poster |
| poster-card-lipsync.webp | first frame of card-lipsync.mp4 | Reel poster |
| poster-card-upscale.webp | first frame of card-upscale.mp4 | Reel and Product pipeline poster |
| poster-card-character.webp | first frame of card-character.mp4 | Reel and Character pipeline poster |
| lastframe-card-character.webp | last frame of card-character.mp4 | Character pipeline, Last frame step |

## Brand (`web/assets/brand`)

| File | Source | Made with |
|---|---|---|
| mark.svg | dd2465f | The play-leaf mark, drawn as SVG in this repo |
| icon.svg | dd2465f | The play-leaf mark on the app tile |
| apple-touch-icon.png | dd2465f | Rendered from the play-leaf mark |

The pipeline pictures are examples from the reel, not the output of a pipeline run, and the page says so.
The display face on the homepage is Anybody (SIL Open Font License), loaded from Google Fonts; Geist and Geist Mono
ship in `web/assets/fonts`.
