# Camera moves

Agave has 20 camera moves. Every move adds its words to the prompt. Video models follow those words some of the
time; Kling 3.0 image to video, the default, often does not (in our tests Dolly in left the camera still twice).

So on models that take an end picture (models.toml `end_frame`, Kling 3.0 image to video), some moves are
**keyframed**: Agave sends the start picture and a picture of where the move ends, and the model animates between
them, so the move should happen even when the words are ignored. Dolly in was checked this way on a paid clip
(2026-10-19: a clear push in after two clips with words alone stood still); the other keyframed moves have not been
checked on clips yet.

| Move | How the end picture is made | Extra cost |
|---|---|---|
| Dolly in | The middle 65% of the start picture, scaled back to its size | none (local) |
| Crash zoom | The middle 40% | none (local) |
| Dolly out | Nano Banana Edit: the same moment from a camera moved far back | that model's list price |
| Orbit left, Orbit right | Seedream 4 Edit: the same moment from a camera orbited about 35 degrees | that model's list price |

The studio says which kind a move is under the Camera chip, and the price before Generate includes the edit.
Every other move is sent as words only, and the studio says so.

How the models were picked (viewpoint tests on one still, 2026-10-19): Nano Banana Edit moved the camera back
cleanly but turned the subject instead of orbiting; Seedream 4 Edit made a real orbit. Neither raised the camera
for a crane shot, so Crane up and Crane down stay words.

Both pictures come from one upright decode of the start picture and leave at exactly its size (a crop, or the edit
scaled to cover the same frame), so they match in size and orientation. Only a real PNG, JPEG or WebP is decoded.
A view move's edit runs on the same fal key as the video. If the edit or the video then fails, the job fails, and
fal still charges for an edit that was made. Push through stays words: its words ask for a move into the next space,
which a crop of the same picture cannot show.
