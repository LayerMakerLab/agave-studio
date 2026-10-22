# Agave Studio: gateway API

Gateway: Rust (axum), binds 127.0.0.1:4170 (test mode: `AGAVE_TEST_PORT`). Serves `web/` as static root (`/` homepage, `/app/` workspace).
Keys: read from env and from `config/keys.env` (gitignored, chmod 600). `config/keys.example.env` lists every slot.
Never return a key value from any endpoint. Never log a key.

Who may call it: the studio's own pages. Every request must name the gateway's own host (no DNS rebinding), a
request from another site is refused (Origin, Sec-Fetch-Site), and every request that changes something (anything but
GET and HEAD) must be JSON and carry this launch's cookie, `agave_launch_<port>`: a random value made at every start,
given to a browser when it loads one of the studio's pages, HttpOnly and SameSite=Strict. A program that only sends
requests to the API gets 403 {"error":"launch_token"}, as does a page opened before a restart (it asks to be
reloaded). This stops scripts and agents that call the API directly; it is not a sign-in, and a program that drives
a browser gets the cookie like any page. GETs need no cookie.

## Endpoints (JSON)
- GET  /api/health -> {"ok":true,"version":"0.1.0","fake_providers":false}
- GET  /api/models -> {"models":[Model]}
  Model = {"id":"fal/kling-v2.1-master-t2v","name":"Kling 2.1 Master","vendor":"Kuaishou","provider":"fal",
           "modality":"video"|"image"|"audio"|"3d","modes":["t2v","i2v","t2i","i2i","tts","i23d"],
           "params":{"aspect":["16:9","9:16","1:1"],"duration":[5,10],"resolution":["720p","1080p"],"audio":[true,false]},
           "key":"FAL_KEY","ready":true|false,"max_refs":0,"max_elements":0,"end_frame":false,
           "price":{"per":"second","usd":0.168,"by_resolution":{},"by_audio":{"on":0.168,"off":0.112}}|null}
  The first value of each params list is the model's default.
- GET  /api/presets -> {"camera":[{"id":"dolly_in","name":"Dolly in","group":"push","keyframe":{"kind":"crop"}}], "styles":[{"id":"film_35","name":"35mm film"}]}
- GET  /api/keys -> {"keys":[{"env":"FAL_KEY","provider":"fal.ai","set":true,"docs":"https://...","reads_only":false}]}
  `reads_only`: a key that only reads an account (FAL_BILLING_KEY), never runs a model, so it doesn't count as a
  provider key for the "add a key" hint.
- POST /api/keys  {"env":"FAL_KEY","value":"..."} -> {"ok":true}   (localhost only; writes config/keys.env; env must be a known slot)
- GET  /api/voices -> {"voices":[{"id":"21m00Tcm4TlvDq8ikWAM","name":"Rachel","category":"premade"}]}
  The voices on the user's ElevenLabs account (needs ELEVENLABS_API_KEY; 412 key_missing without it). Text to speech
  (`elevenlabs/multilingual-v2`) needs a `voice_id` from this list in POST /api/generate.
- POST /api/generate {"model":"fal/...","mode":"t2v","prompt":"...","camera":"dolly_in","style":"film_35",
                      "aspect":"16:9","duration":5,"resolution":"1080p","seed":null,"image_url":null}
  -> 202 {"job":Job}   | 412 {"error":"missing_key","key":"FAL_KEY"} | 400 {"error":"..."}
- GET  /api/jobs -> {"jobs":[Job]}      GET /api/jobs/:id -> {"job":Job}
  Job = {"id":"j_...","model":"...","mode":"t2v","prompt":"<final prompt>","status":"queued"|"running"|"done"|"failed",
         "progress":0..1|null,"outputs":[{"url":"...","kind":"video"|"image"|"audio"|"model"}],"error":null|"...",
         "created_at":unix,"finished_at":unix,"cost":Cost}   (finished_at once it is done or failed; absent on jobs from before it)
  Cost = {"estimate_usd":0.56,"estimate":"exact"|"about"|"up to","how":"5 s","unit_price":0.112,"unit":"second",
          "average":0.0024,"credits":5,"test":false}   on jobs that go to a provider (absent on the studio's own local
  jobs: edits, frames). `estimate_*` and `how` are what the page showed before Run; `unit_price`/`unit` are the
  provider's list price for the job's options (`average`: fal's own average per call; `credits`: a provider that
  prices in its own credits). Fields with no value are left out. POST /api/generate and POST /api/fal/run take an
  optional `"estimate":{"usd":0.56,"kind":"about","how":"5 s"}`; one the page could not have shown (the wrong shape,
  negative, over $1000, another kind) is left out, never refused, and `how` keeps only plain words. Pipeline steps carry
  the price their page shows.
  A fal job also keeps `request_id` (fal's id for the request), and once fal's billing reports it, `charged_usd` and
  `charged_nano` (what fal charged, in dollars and in fal's exact nano dollars). See "What fal charged".

## Spend log
`data/spend.jsonl` (test mode: `data/fake-providers/spend.jsonl`, every line `"test":true`): one line per job a
provider took (it gave the job a handle, or answered it at once), written when the job ends, done or failed:
`{"at":unix,"job":"j_...","model":"fal:fal-ai/flux/schnell","status":"done"|"failed","cost":Cost}`.
A job refused before the provider took it (a missing file, a refusal at submit) adds no line. A job the provider had
given a handle when the studio closed adds its failed line on the next start when it can't be picked up again (too
old, or its model gone). A job whose submit was still in flight when the studio closed adds no line, though the
provider may have charged for it. A provider that answers at once (OpenAI and Gemini images, ElevenLabs) is counted
when its answer is saved: if the answer can't be read or saved (a full disk), the job fails with no line, though the
provider may have charged for it too. Owner-only (0600), appended, never rewritten, and never written through
a symlink, into a hard link to another file, or into a FIFO. The amounts are estimates from list prices; the
provider's own bill is what was charged (a failed job may or may not have been). A keyframed camera move pays for its
picture edit before the video is sent: if the video is then refused, that edit is not in the log. A crash between a
job's line and data/jobs.json can repeat its line after a restart, so a reader keeps the last line per `job`.

## What fal charged
With `FAL_BILLING_KEY` set (a fal key made with the BILLING preset: it reads usage and billing and cannot run
models), the studio looks up what fal charged for its finished fal jobs. fal reports a request some minutes after it
ends, so jobs that ended in the last 7 days without a charge (failed ones: the last day, since fal rarely bills them)
are looked up again: every 10 minutes while any wait, and when the Spending panel asks. Newest first, 50 request ids
per lookup (fal's limit), at most 500 jobs at once, every page of fal's answer read (a lookup with more than 10 pages
records nothing; the lookups after it still run, unless fal is busy or refused the key). Each charge goes on the job's `cost` and adds a fresh spend line for the job with the charge next to
the estimate (the last line per `job` is the one to keep). No key, no lookups; test mode never asks fal. A run fal
hasn't billed within the 7 days keeps its estimate only. A charge is recorded once: billing rows fal adds for the
same request later aren't. Only the request that made the job's result is looked up, so a keyframed camera move's
picture edit (its own request, inside the page's estimate) is not in the charge.

- GET  /api/spending -> {"billing_key":true,"test":false,"week":[{"model":"fal/kling","name":null,"runs":3,
                         "estimate_usd":1.68,"unestimated":0,"charged_usd":1.12,"billable":3,"uncharged":1}]}
  The last 7 days of the studio's own paid runs by model, from data/jobs.json (no call to fal): done runs, and failed
  runs fal charged for. `unestimated`: runs with no estimate; `billable`: runs fal can bill (fal jobs that kept fal's
  request id; not other providers' runs or runs from before this); `uncharged`: billable runs fal hasn't billed yet.
  `name`: a Playground model's own name. Most charged first.
- POST /api/spending/refresh -> {"ok":true,"found":2,"error":null,"fal":{"total_usd":4.12,"by_endpoint":[{"endpoint":
                                 "fal-ai/kling-video/...","usd":3.36}]}}
  Looks up waiting charges now (`found`: how many jobs got theirs, also when a later lookup failed; `ok:false` with
  `error` "fal_busy", "key_refused" (fal refused FAL_BILLING_KEY) or "fal_billing_unavailable" when fal couldn't be
  asked), then reads fal's own total for the last 30 days by endpoint
  (top 20). That total is the whole fal account, not only this studio; `fal` is null when fal doesn't answer. Without
  the key, or in test mode: {"ok":true,"found":0,"fal":null}. Needs the page's launch cookie like every change.

## Building blocks
- POST /api/uploads {"data":"data:<any>;base64,..."} -> 201 {"url":"/outputs/<hex>.<ext>","kind":"image"|"video"|"audio"}
  | 400 data_uri_required/bad_base64 | 413 file_too_large (40 MB) | 415 unsupported_file. The type is read from the
  bytes (png, jpg, webp, mp4, mov, webm, mp3, wav, m4a), never from the data: header.
- GET  /api/elements -> {"elements":[{"tag","name","kind","images":["/outputs/..."],"created_at"}]}
- POST /api/elements {"tag","name","kind","images"} -> saves (same tag replaces). kind = character|product|location|prop|style;
  1-4 local /outputs images; max 200 elements. 400 invalid_tag/invalid_name/invalid_kind/invalid_image | 409 too_many_elements
- POST /api/elements/delete {"tag"} -> {"ok":true} | 404 not_found
- GET/POST /api/project {"style_prefix":"...","still_first":true} -> {"project":{...}} (data/studio.json; prefix max 800 chars)
- POST /api/generate also takes "image_urls":[...] and "recipe":"grid4"|"grid9"|"edit"|"sheet"|"product"|"look"
  ("look": the picture in the chosen style's look, no words needed; the studio offers it before image to video with a
  look, since the start picture sets a video's look), and "audio":true|false for models whose params list `audio`
  (Kling 3.0, Kling O3, Veo 3.1; the first value is the default and the price follows `price.by_audio`).
  `@tag` in the prompt becomes the Element's words and its images join the references (not after a letter or digit,
  so emails stay as typed). References = image_url, then image_urls, then Element images. Models with `max_refs` > 1
  (models.toml) get all of them as `image_urls`; others get the first as `image_url`. Errors: 400 {"error":"too_many_refs",
  "max","got"} | 400 refs_need_image_mode (text-only mode with references) | 400 unknown_recipe | 400 prompt_required
  (edit recipe with no change) | 409 still_first (t2v while the project's still-first rule is on; make a still, then
  i2v). Models with `max_elements` (Kling 3.0 image to video) take a start picture and @tagged Elements side by side:
  each Element with pictures becomes @Element1, @Element2... in the words and goes to fal as `elements` (400
  {"error":"too_many_element_tags","max","got"} past the limit). Models with `end_frame` (Kling 3.0 image to video)
  get keyframed camera moves (docs/CAMERA.md): Dolly in and Crash zoom send the start picture and its crop; Dolly
  out and Orbit left/right first make the end picture with an image edit on the same key. GET /api/presets
  marks each camera move with "keyframe": {"kind":"crop"} | {"kind":"view","model":"<edit model id>"} | null (words).
  A "look" recipe keeps the picture's own frame (no aspect is sent). The project style prefix is put in front of image
  and video prompts. The job keeps its request as Job.request (when 16 KB or less) for "Run again".
- POST /api/assemble (Agave Edit render) {"clips":[clip,...],"captions":[null|"/outputs/cap.png",...],
  "audio":null|"/outputs/song.mp3","keep_clip_audio":true,"frame":null|"16:9"|"9:16"|"1:1"|"4:5","fill":false}
  -> 202 {"job":Job} (model "agave/editor"). A clip is a bare "/outputs/x.mp4" (whole clip) or
  {"url","from","to"} (seconds inside the source; `to` null = to the end). A picture (png/jpg/webp) is a still that
  shows for `to - from` seconds (default 3, max 60). 1-200 clips. Local ffmpeg: each piece is trimmed, framed to the
  output size (frame, else the first clip's shape capped at 1920) by fitting with bars or, with `fill`, cropping,
  30 fps; its caption PNG (drawn by the app at the output size) goes on top; then all pieces are joined. A soundtrack
  replaces the sound and is cut to the video's length. 400 invalid_clip (missing file, wrong type, or a trim that
  runs backwards)/invalid_frame/too_many_captions/invalid_caption/invalid_audio/clips_1_to_200.
  Optional "timed_captions":[{"url":"/outputs/c.png","start":1.2,"end":1.6},...] (auto captions): caption PNGs drawn
  by the app at the output size, sorted, not overlapping, up to 3000; laid over the joined edit in one pass (an
  ffconcat image list with a clear image in the gaps). 400 invalid_timed_captions/too_many_timed_captions.
  Optional "follow":true (with `fill`): the crop follows the subject instead of the middle (see /api/reframe); a clip
  the tool can't read falls back to the middle. Optional "clean_voice":true: highpass 80 Hz + anlmdn (steady hiss
  down about 15 dB in the test). Optional "loudness":true: two-pass loudnorm to -14 LUFS, true peak -1.5 dB, LRA 11,
  48 kHz (the common target for YouTube and short-form apps; not a figure YouTube publishes).
- POST /api/reframe {"url":"/outputs/x.mp4","frame":"9:16"} -> {"axis":"x"|"y"|"none","keys":[[t,pan],...],"tracked"}
  (t in source seconds, pan 0..1 along the axis; the app uses it for the preview and the render uses the same path).
  `native/vision/agave-vision` (Swift, Apple Vision, built by native/vision/build.sh; or AGAVE_VISION_BIN) samples 4
  frames a second: faces, else people, else objectness saliency, else attention saliency. ffmpeg scene detection
  (score 0.3) finds the source's shot changes. The camera holds inside a 0.12 dead zone, eases toward a subject that
  leaves it, and cuts on a shot change (keyed half a frame early so no frame shows the old framing). Cached in
  data/tracks/. No tool = tracked:false, keys []. 400 invalid_file/invalid_frame.
- POST /api/transcribe {"url":"/outputs/x.mp4"} -> {"transcript":{"language","words":[{"t0","t1","text"}]}} (seconds in
  the source). Engine: NVIDIA Parakeet TDT 0.6B v3 when `native/speech/agave-speech` (Swift, FluidAudio 0.17.7, built by
  native/speech/build.sh; or AGAVE_SPEECH_BIN) and data/models/parakeet-tdt-0.6b-v3 are installed (language is ""; 25
  European languages); else local whisper.cpp: `whisper-cli` on PATH and data/models/ggml-small.bin (or AGAVE_WHISPER_MODEL). The
  audio is cut at pauses (silencedetect -35 dB, 0.3 s) and each stretch of speech is transcribed alone, so words never
  land in a pause; sound labels like "(upbeat music)" are dropped. One at a time; cached in data/transcripts/.
  503 captions_unavailable (no whisper or model), 400 invalid_file/too_long (over 30 min), 502 transcribe_failed.
- GET /api/setup -> {"speech":{"engine":"parakeet"|"whisper"|null,"parakeet_tool","whisper_tool"},"models":[{id,name,engine,languages,source,
  size,progress:{state:"idle"|"paused"|"downloading"|"checking"|"preparing"|"ready"|"failed",done,total,error}}]}
  ("paused": bytes from a cancelled or broken download are on disk and will be reused). POST
  /api/setup/download {"model":"parakeet-v3"|"whisper-small"} -> 202 {"progress"} starts or resumes a background download;
  POST /api/setup/cancel {"model"}. Every file is pinned in gateway/src/setup.rs (Hugging Face revision, size, SHA-256),
  downloaded to data/models/.<id>.part (Range resume), checked, then moved into place; a file that fails its check is
  deleted (error file_check_failed). Free space is checked first (not_enough_space). Downloads follow redirects only to
  huggingface.co / hf.co hosts. Parakeet is then loaded once ("preparing", about 30 s) so the first caption job is fast.
  400 unknown_model, 503 speech_tool_missing. The app's first-launch sheet (#setup:
  fal.ai key, speech model, summary) opens when no FAL_KEY or no speech engine is set and it has not been closed before
  (localStorage agave.setup.v1); Settings > Setup reopens it.
- POST /api/pauses {"url","threshold_db":-35,"min_pause":0.5} -> {"len","has_audio","pauses":[[start,end],...]}
  (ffmpeg silencedetect; -60..-20 dB, 0.2..5 s, else 400 invalid_settings). The app cuts these out of each video
  clip, keeping 0.12 s of silence on each side, and offers Undo.
- GET /api/edit -> {"edit":EditDoc,"rev":n}; POST /api/edit EditDoc (+ optional "rev") -> {"edit":EditDoc,"rev":n}.
  `rev` goes up by one on every save (the page's own saves and a pipeline's Add to edit). A POST that carries the rev
  it last read is refused with 409 {"error":"edit_changed","edit":<the saved timeline>,"rev":n} when the timeline has
  changed since, so a page holding an older copy cannot erase newer clips. The studio then rebases: it keeps every
  local change, appends the clips that are new in the saved timeline, says so, and saves again with the new rev. A
  POST without rev saves as before. The timeline is saved in data/studio.json, so an edit survives a reload or restart. EditDoc {clips:[{url,kind:"video"|"image",from,to,len,caption}],
  frame:"original"|"16:9"|"9:16"|"1:1"|"4:5", fill, keep_sound, song_url, song_name,
  auto_captions:"off"|"simple"|"pop", word_fixes:[{from (one word), to ("" hides it)}] (up to 50), follow,
  clean_voice, loudness}. Export buttons: "For YouTube" = 16:9 Fit + loudness; "For Shorts, TikTok, Reels" = 9:16
  Follow + loudness (warns over 3 minutes, the Shorts limit). Only the shape is checked here
  (local /outputs URLs, numbers >= 0, captions <= 120 chars); files are checked again at render.

## 3D and the prompt library
- Model modality "3d", mode "i23d": `meshy/image-to-3d` (key MESHY_API_KEY). 1 picture goes to Meshy's Image to 3D, 2-4 to
  Multi-Image to 3D (meshy-7.1 "latest", 4K PBR textures, formats glb, usdz, stl, 3mf). Words are optional and only name the
  job (no texture_prompt, which Meshy bills separately). Outputs: the four model files (kind "model", GLB first) then
  Meshy's thumbnail (kind "image"). No USD price is published, so the model has `provider_credits = 30` (Meshy's price
  list) and no `price`; the app shows "30 Meshy cr".
- GET /api/models also returns `provider_credits` (null for most models).
- The app's Library reads `web/app/library.json` (static): id, category, title, blurb, mode, model, aspect, recipe,
  needs_ref, prompt, preview (under /app/), preview_kind, preview_note. "Use this" fills the composer; nothing runs.

## Pipelines
Several existing steps in a row (gateway/src/pipelines.rs): a picture, Animate, Continue from the last frame, Save as
Element, 3D, Add to edit. Each step is fed by the one before; runs live in data/pipelines.json (owner-only), so a run
survives a reload or a restart. Generation steps go through the same checks as POST /api/generate.
- GET /api/pipelines -> {"pipelines":[{id,title,blurb,note,needs_image,image_hint,subject_hint,tag,element_kind,
  steps:[{title,kind:"generate"|"element"|"last_frame"|"edit",paid,model,model_name,vendor,price:{cents,credits,
  credits_vendor}}],total:{cents,credits:[{vendor,credits}],unpriced,paid_steps},keys:[{env,set}]}]}
  Prices are the models.toml list prices with the composer's rounding (rate x seconds, or per picture, rounded up to
  the cent per step); Meshy steps count credits.
- GET /api/pipelines/runs -> {"runs":[Run]} newest first (moves running runs on first).
  Run = {id:"p_<16 hex>",pipeline,title,created_at,subject,image,tag,status:"waiting"|"running"|"failed"|"done",current,
         steps:[...step fields above, status:"pending"|"waiting"|"running"|"done"|"failed",job,outputs,error,key],total,removed}
  `removed` is true for a run of a pipeline that is no longer in the studio: its title and total are null, its steps
  show as "Step N" with kind "removed", its files stay, and it cannot go on (a step in flight fails with a note).
- POST /api/pipelines/runs {"pipeline","subject" (1-60 chars, also the Element's words),"image":"/outputs/..." (when
  needs_image),"tag"} -> 201 {"run"} | 400 unknown_pipeline/invalid_subject/image_required/invalid_image/invalid_tag |
  409 tag_in_use (that @tag is already an Element) | 409 tag_in_run (another run that is not finished holds it; a failed
  run can still be run again, so it keeps its tag until it finishes or is forgotten). A paid first step waits; free
  steps start at once. The run records the Element it saved (`element`), and its Save as Element step fails, leaving
  the Element alone, when the tag now belongs to an Element the run did not make.
- POST /api/pipelines/runs/{id}/step {"step":n} -> {"run"}: the user's click. Only the current step, when it is
  waiting or failed (409 step_not_ready otherwise; 409 pipeline_changed for a run saved by an older version of its
  pipeline, which is failed instead of resumed; 409 unknown_pipeline for a `removed` run). A refused request (for example 412 missing_key) is recorded on the
  step as `error` (and `key`), so the answer is the run either way. A failed step stops the run; clicking it again runs
  only that step and keeps everything before it.
- POST /api/pipelines/runs/{id}/delete {} -> {"ok":true} | 409 run_busy (a step is waiting on a job). Files and the
  Element stay. At most 50 runs are kept: finished runs go first, oldest first, then the others; a run with a job in
  flight is never dropped.
- A step can carry a `note` (for example "Added 1 of 2 clips: the timeline holds 200.").
Paid steps (every generation bills your own provider account) never start on their own. Free steps (Save as Element,
the last frame, Add to edit) run as soon as the step before them is done; a background check
every 2 s moves runs on with no page open. The last frame is taken by ffmpeg as a local job (model "agave/last-frame",
shown in the gallery like other jobs), so a restart in the middle fails that step instead of losing it.

## Boards
The studio's Board view (`web/app/board.js`, the Excalidraw editor vendored in `web/vendor/excalidraw/`) saves each
board as one Excalidraw file, `data/boards/<name>.excalidraw` (test mode: `data/fake-providers/boards/`), owner-only
(0600 in a 0700 folder) and written atomically (a new file renamed over the old one, so a planted symlink is replaced,
never followed). Pictures on a board are embedded in its file. A board name is also its file name: 1 to 60 of A-Z,
a-z, 0-9, space, `-`, `_`, `(`, `)`, starting with a letter or digit, not ending in a space; names that differ only in
letter case are the same board (macOS folders ignore case). Symlinks, hard links and other files in the folder are
not boards. At most 500 boards, 25 MB each. Every route goes through the same guard as the rest of the API; saving,
renaming and deleting need the page's launch cookie.
- GET  /api/boards -> {"boards":[{"name":"Ep 1","bytes":21356,"modified":unix,"rev":"<16 hex>"}]} most recently saved
  first. `rev` is the first 16 hex of the file's SHA-256.
- GET  /api/boards/load?name=Ep%201 -> {"name","rev","bytes","modified","board":{"type":"excalidraw","version":2,
  "elements":[...],"appState":{...},"files":{...}}} | 400 invalid_name | 404 not_found | 400 invalid_board (the file
  is not an Excalidraw scene) | 413 board_too_large | 500 read_failed (the file could not be read).
- POST /api/boards/save {"name","board","create":false,"rev":null} -> {"board":{name,bytes,modified,rev}}. `board`
  must be an Excalidraw scene: an object with "type":"excalidraw" and an `elements` array (`appState` and `files`
  objects optional); it is saved as compact JSON, and the 25 MB limit is measured on that, as the page measures it.
  `create`: only when no board has that name (409 board_exists, also for a name that differs only in case). `rev`:
  only when the saved board still has that rev (409 {"error":"board_changed","rev":"<saved rev>"}), and 404 not_found
  when no board has that name any more (it was renamed or deleted elsewhere; the page asks before saving it again);
  without it the save replaces what is saved. 400 invalid_name/invalid_board/bad_request | 409 too_many_boards | 413
  {"error":"board_too_large","max_mb":25} (the body limit is 25 MB plus 64 KB) | 500 write_failed.
- POST /api/boards/rename {"from","to"} -> {"board":{...}} | 404 not_found | 409 board_exists (a change of letter case
  alone is allowed) | 400 invalid_name | 400 bad_request (not the expected JSON, or a body over 2 MB).
- POST /api/boards/delete {"name"} -> {"ok":true} | 404 not_found | 400 invalid_name | 400 bad_request (as for rename).
In the studio: results go on a board as pictures shrunk to 1600 px on the long side and about 1.9 MB at most (WebP, or
JPEG where the browser cannot write WebP); a video goes on as a still from 0.5 s in (a third of the way into shorter
clips; a plain card if no frame can be read within 10 s), a 3D model as its thumbnail, and either (or a result with no
picture) links back to its file and has a label grouped with it. Each frame on a board is a shot: its words (text
inside the frame, top to bottom and left to right, labels left out) are the prompt and its first picture is the start
picture (the result's own /outputs file when it came from the studio, else the board's copy, saved through POST
/api/uploads). "Send to Video" fills the Video form with them and runs nothing; the job starts only with Generate. A
board shows only embedded pictures (PNG, JPEG, WebP, GIF or SVG data: URLs): a picture whose file is anything else,
every iframe and every web embed are left out when a board opens, is dropped on the canvas or changes, and the page
says so. A paste with pictures from the web or embeds is refused, the web-embed tool and the library's Browse
libraries link are hidden, and a picture's link opens only when it is an /outputs/ file.

## Playground (any fal model)
- GET  /api/fal/models?q=&category=&cursor= -> {"models":[{"id":"fal-ai/flux/schnell","name":"...","category":"text-to-image",
       "description":"...","thumb":"https://..."|null,"makes_files":true}],"next_cursor":"..."|null,"has_more":bool,
       "categories":[{"id":"text-to-image","label":"Text to image"}]}
  One page (40) of fal's active models, from api.fal.ai/v1/models, cached 10 minutes. `category` must be one of the
  listed ids (400 unknown_category; 400 bad_query for a query over 100 characters or a malformed cursor). 503 catalog_busy
  when fal rate-limits, 502 catalog_unavailable when it cannot be reached. Test mode adds `"test":true`.
- GET  /api/fal/model?id=<endpoint> -> {"model":Spec}
  Spec = {"id","submit_path","name","category","description","status","thumb","page","runnable":bool,
          "why_not":"..."|null,"note":"..."|null,"outputs":["image"|"video"|"audio"|"model"|"file"],"fields":[Field]}
  `submit_path` is the POST path in fal's schema, where runs go (for a few models it differs from the catalog id).
  400 bad_model_id, 404 unknown_fal_model, and the catalog's 503 and 502.
  Field = {"name","title","description","kind":"text"|"longtext"|"integer"|"number"|"boolean"|"choice"|"media"|
           "medialist"|"json","media":"image"|"video"|"audio"|"file"|null,"required","default","example","choices",
           "custom_json","min","max","min_strict","max_strict","max_len","advanced","limits"}
  Compiled from the model's OpenAPI input schema (cached 6 hours). `sync_mode` is never offered, and neither is an
  `output_format` that chooses between "url" and inline data: runs always send "url" (naming it with any other value is
  400 invalid_value), since an inline answer leaves no file to keep. An enum beside a free
  string or number keeps the field free, with the enum in `choices` as suggestions. A model is runnable when its result
  schema holds files; training models and retired ones are not.
- GET  /api/fal/price?id=<endpoint> -> {"price":{"unit_price":0.07,"unit":"seconds","currency":"USD","average":0.42|null,
       "checked_at":unix,"test":false}}
  fal's list price (api.fal.ai/v1/models/pricing) and fal's own average cost of one call (pricing/estimate,
  historical_api_price), cached 1 hour. Any of unit_price, unit, currency and average can be null. Needs FAL_KEY
  (412 {"error":"missing_key","key":"FAL_KEY"}); 502 price_unavailable when fal gives neither.
- POST /api/fal/run {"id":"<endpoint>","input":{...},"test":bool?} -> 202 {"job":Job}
  The input is checked against the Spec: unknown fields, wrong types, values outside the schema's range or choices,
  and missing required fields are refused with 400 {"error":"unknown_field"|"invalid_value"|"field_required","field":"..."}.
  Nulls are dropped (fal's default applies). Text takes up to the field's maxLength (20,000 characters when the schema
  gives none, never more than 200,000), a JSON field up to 64 KB, a file list up to 16 items. A free text or number
  field also takes the values of an enum fal lists beside it. The request body is capped at 2 MB (400
  bad_request), so send bigger files through POST /api/uploads. File fields take https URLs, data: URIs, or
  /outputs/<file>; local files in file and JSON fields go to fal inline, up to 20 MB each and 40 MB a run
  (413 {"error":"file_too_large"|"files_too_large","max_mb":n}; 400 file_missing). Other errors: 400 not_runnable (with
  "why"), 412 missing_key (with "key"), 409 not_test_mode (the page said test mode but the studio is not in it),
  429 too_many_jobs, and the /api/fal/model errors. The job's model is "fal:<endpoint>", its mode the fal category,
  and its request {"playground":{"id","input"},"name","price"} (kept when 16 KB or less, as for /api/generate).
  Up to 24 files in fal's result (from at most 200 file links, the most central first) are saved under /outputs when
  they are a picture, video, audio or 3D model (never SVG or HTML), up to 512 MB each and 2 GB a run; other files, and
  links that are not https, are skipped. Every result file is fetched only from a host that resolves to public addresses, pinned to the
  checked addresses, with redirects refused, and tried once more a second later when it fails for a reason that may
  pass (network, HTTP 5xx, 408, 429); not public, too large, a redirect and other HTTP 4xx do not pass. Files rank 3D
  model, video, audio, picture, and the most central file that failed decides: one less central than a saved file (a
  preview beside the video) is let go; one as central or more that may pass makes the poll remove what it saved and
  run again (3 polls at most, counted from the start again after a restart; these polls count toward the studio's
  limit of 5 failed polls in a row); after that, or when it cannot pass, a more central file still missing fails the
  job with the reason, and one of several equal files is let go. Test mode answers voice and sound models with a short tone.

## Test mode
`AGAVE_FAKE_PROVIDERS=1` at startup (off by default): every model is answered by a local fake (a placeholder picture
that says TEST OUTPUT and names the step, and an ffmpeg clip of it for video), with no key and no network; data goes to data/fake-providers (its own jobs,
Elements, edit and keys file). `AGAVE_FAKE_FAIL=video,3d,image` makes the first job of each listed kind fail once, to try
Run again. GET /api/health reports `fake_providers` and the studio shows a banner. The Playground reads seven real fal
models captured in `gateway/playground-sample.json` instead of the network. `AGAVE_TEST_PORT=4171` runs a test-mode
gateway on another local port, beside a real one: a port from 1024 up (anything else stops startup), ignored outside
test mode.

## Prompt craft (C++)
`engine/promptcraft` (C++17) exposes extern "C" `const char* pc_compose(const char* prompt, const char* camera_id, const char* style_id)`
and `void pc_free(const char*)`. Linked into the gateway via the `cc` crate. It appends the camera-move and style
language to the user's prompt. Preset tables live in C++.

## Design
Use `web/css/tokens.css` (graphite, silver, one ember accent). Primary button = `.pill`.
Fonts: Geist, Geist Mono, Instrument Serif, served from `web/assets/fonts` (`web/css/fonts.css`); the homepage alone also
loads Anybody from Google Fonts. The board's editor brings its own fonts in `web/vendor/excalidraw/fonts`.
