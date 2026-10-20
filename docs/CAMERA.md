# Camera moves

Agave has 20 camera moves. Every move adds its words to the prompt. Video models follow those words some of the
time; Kling 3.0 image to video, the default, often does not (in our tests Dolly in left the camera still twice).

So on models that take an end picture (models.toml `end_frame`, Kling 3.0 image to video), some moves are
**keyframed**: Agave sends the start picture and a picture of where the move ends, and the model animates between
them. The move happens because the end picture is already there.

| Move | How the end picture is made | Extra cost |
|---|---|---|
| Dolly in | The middle 65% of the start picture, scaled back to its size | none (local) |
| Push through | The middle 55% | none (local) |
| Crash zoom | The middle 40% | none (local) |
| Dolly out | Nano Banana Edit: the same moment from a camera moved far back | that model's list price |
| Orbit left, Orbit right | Seedream 4 Edit: the same moment from a camera orbited about 35 degrees | that model's list price |

The studio says which kind a move is under the Camera chip, and the price before Generate includes the edit.
Every other move is sent as words only, and the studio says so.

How the models were picked (viewpoint tests on one still, 2026-10-19): Nano Banana Edit moved the camera back
cleanly but turned the subject instead of orbiting; Seedream 4 Edit made a real orbit. Neither raised the camera
for a crane shot, so Crane up and Crane down stay words.

Zoom moves decode the start picture once, upright, and send that frame and its crop, so both pictures always
match in size and orientation. Only a real PNG, JPEG or WebP is decoded. A view move's edit runs on the same
provider key as the video; if it fails, the job fails before the video is sent.
