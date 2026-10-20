//! Camera moves that Agave keyframes with an end picture, for image to video models that take one (models.toml
//! `end_frame`, Kling 3.0). Kling often ignores camera words; given the start picture and where the move ends, it
//! animates between the two. Zoom moves crop the start picture (free, local); moves that change the viewpoint make
//! the end picture with an image edit on the same key, priced before Generate. Every other move stays words.
//! Which moves are keyframed, and how the edit models were picked, is in docs/CAMERA.md.

/// How a camera move gets its end picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Keyframe {
    /// The middle of the start picture, this fraction of its width and height, scaled back to its size.
    Crop(f64),
    /// An image edit of the start picture by `model` (a models.toml id) with `prompt`.
    View { model: &'static str, prompt: &'static str },
}

const KEEP: &str = "Keep the exact same scene, the same people, outfits, poses, lighting, colours and time of day. \
                    Only the camera position changes.";

const DOLLY_OUT: &str = "Show the same moment from a camera that has moved far back, so the main subject is much \
                         smaller and more of the place is visible around it.";
const ORBIT_LEFT: &str = "Show the same moment from a camera that has orbited about 35 degrees to the left around \
                          the main subject, so we see the subject and the place from a new angle, with matching \
                          perspective.";
const ORBIT_RIGHT: &str = "Show the same moment from a camera that has orbited about 35 degrees to the right around \
                           the main subject, so we see the subject and the place from a new angle, with matching \
                           perspective.";

/// The end-picture plan for a camera preset id, or None when the move stays words.
pub fn keyframe(camera: &str) -> Option<Keyframe> {
    Some(match camera {
        "dolly_in" => Keyframe::Crop(0.65),
        "crash_zoom" => Keyframe::Crop(0.4),
        // Picked from viewpoint tests on 2026-10-19: Nano Banana Edit moved the camera back cleanly; Seedream 4 Edit
        // made a real orbit where Nano Banana turned the subject instead.
        "dolly_out" => Keyframe::View { model: "fal/nano-banana-edit", prompt: DOLLY_OUT },
        "orbit_left" => Keyframe::View { model: "fal/seedream-4-edit", prompt: ORBIT_LEFT },
        "orbit_right" => Keyframe::View { model: "fal/seedream-4-edit", prompt: ORBIT_RIGHT },
        _ => return None,
    })
}

/// The full edit prompt for a view move.
pub fn view_prompt(prompt: &str) -> String {
    format!("{prompt} {KEEP}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_moves_crop_view_moves_edit_the_rest_stay_words() {
        assert_eq!(keyframe("dolly_in"), Some(Keyframe::Crop(0.65)));
        assert!(matches!(keyframe("crash_zoom"), Some(Keyframe::Crop(c)) if c < 0.65));
        assert!(matches!(keyframe("dolly_out"), Some(Keyframe::View { model: "fal/nano-banana-edit", .. })));
        assert!(matches!(keyframe("orbit_right"), Some(Keyframe::View { model: "fal/seedream-4-edit", prompt }) if prompt.contains("right")));
        for words in ["crane_up", "push_through", "whip_pan", "handheld", "static_lockoff", "nope"] {
            assert_eq!(keyframe(words), None, "{words}");
        }
        assert!(view_prompt(DOLLY_OUT).ends_with("Only the camera position changes."));
    }
}
