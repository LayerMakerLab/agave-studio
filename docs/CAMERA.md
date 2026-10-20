# Camera moves

Agave has 20 camera moves. Every move adds its words to the prompt. Video models follow those words some of the
time; Kling 3.0 image to video, the default, often does not (in our tests Dolly in left the camera still twice).

So on models that take an end picture (models.toml `end_frame`, Kling 3.0 image to video), some moves are
**keyframed**: Agave sends the start picture and a picture of where the move ends, and the model animates between
them, so the move should happen even when the words are ignored.

| Move | How the end picture is made | Extra cost |
|---|---|---|
| Dolly in | The middle 65% of the start picture, scaled back to its size | none (local) |
| Crash zoom | The middle 40% | none (local) |
| Dolly out | Nano Banana Edit: the same moment from a camera moved far back | that model's list price |
| Orbit left, Orbit right | Seedream 4 Edit: the same moment from a camera orbited about 35 degrees | that model's list price |

The studio says which kind a move is under the Camera chip, and the price before Generate includes the edit.
Every other move is sent as words only, and the studio says so.

Each keyframed move was checked on one paid clip (Kling 3.0 Pro image to video, audio off, 2026-10-19). One clip
each is a first check, not a guarantee:

| Move | Clip | What happened |
|---|---|---|
| Dolly in | 5 s, a dancer | a clear push in (two clips with words alone had stood still) |
| Crash zoom | 3 s, a lighthouse in a storm | a strong push in over the whole clip, more a fast dolly than a snap zoom |
| Dolly out | 3 s, a courier on a bike in an alley | a clear, smooth pull back; the rider stays whole |
| Orbit left | 3 s, a dancer in an empty hall | a real arc around her, about 15 degrees, smaller than the edit's 35 |

Orbit right uses the same edit as Orbit left with the direction swapped and has not had its own clip.

How the models were picked (viewpoint tests on one still, 2026-10-19): Nano Banana Edit moved the camera back
cleanly but turned the subject instead of orbiting; Seedream 4 Edit made a real orbit. Neither raised the camera
for a crane shot, so Crane up and Crane down stay words.

Both pictures come from one upright decode of the start picture and leave at exactly its size (a crop, or the edit
scaled to cover the same frame), so they match in size and orientation. Only a real PNG, JPEG or WebP is decoded.
A view move's edit runs on the same fal key as the video. If the edit or the video then fails, the job fails, and
fal still charges for an edit that was made. Push through stays words: its words ask for a move into the next space,
which a crop of the same picture cannot show.
