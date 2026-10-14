#include "promptcraft.h"

#include <cctype>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

struct Camera {
    const char* id;
    const char* name;
    const char* group;
    const char* phrase;
};

struct Style {
    const char* id;
    const char* name;
    const char* phrase;
};

const Camera kCameras[] = {
    {"dolly_in", "Dolly in", "push", "slow dolly in toward the subject"},
    {"dolly_out", "Dolly out", "pull", "slow dolly out, revealing the wider scene"},
    {"orbit_left", "Orbit left", "orbit", "camera orbits left around the subject"},
    {"orbit_right", "Orbit right", "orbit", "camera orbits right around the subject"},
    {"crane_up", "Crane up", "crane", "crane shot rising up and away from the ground"},
    {"crane_down", "Crane down", "crane", "crane shot descending toward the subject"},
    {"whip_pan", "Whip pan", "pan", "fast whip pan with motion blur"},
    {"crash_zoom", "Crash zoom", "zoom", "sudden crash zoom into the subject"},
    {"dolly_zoom", "Dolly zoom", "zoom", "dolly zoom (vertigo effect), background stretching while the subject holds its size"},
    {"rack_focus", "Rack focus", "focus", "rack focus shifting from foreground to background"},
    {"handheld", "Handheld", "handheld", "handheld camera with natural shake and micro-movement"},
    {"fpv_dive", "FPV dive", "aerial", "fast FPV drone dive through the scene"},
    {"overhead", "Overhead", "aerial", "straight-down overhead shot"},
    {"low_angle_orbit", "Low-angle orbit", "orbit", "low-angle camera orbiting the subject from ground level"},
    {"tilt_reveal", "Tilt reveal", "pan", "camera tilts up to reveal the subject"},
    {"bullet_time", "Bullet time", "special", "bullet time, camera arcing around a frozen moment"},
    {"snorricam", "Snorricam", "special", "snorricam rig, camera locked to the subject while the world moves around them"},
    {"tracking_side", "Tracking side", "tracking", "side tracking shot moving alongside the subject"},
    {"push_through", "Push through", "push", "camera pushes through an opening into the next space"},
    {"static_lockoff", "Static lock-off", "static", "locked-off static camera, no movement"},
};

const Style kStyles[] = {
    {"film_35", "35mm film", "shot on 35mm film, natural grain, warm highlights"},
    {"anamorphic", "Anamorphic", "anamorphic lens, horizontal lens flares, shallow depth of field, oval bokeh"},
    {"documentary", "Documentary", "documentary realism, natural available light, observational framing"},
    {"noir", "Film noir", "film noir lighting, hard shadows, low-key contrast"},
    {"commercial_gloss", "Commercial gloss", "polished commercial look, clean high-key lighting, glossy highlights"},
    {"music_video", "Music video", "stylized music video look, saturated colors, dynamic lighting"},
    {"vhs", "VHS", "1980s VHS look, analog noise, chroma bleed, scanlines"},
    {"golden_hour", "Golden hour", "golden hour sunlight, warm backlight, long soft shadows"},
    {"studio_product", "Studio product", "seamless studio backdrop, controlled softbox lighting, crisp product detail"},
    {"anime", "Anime", "cel-shaded anime style, clean line art, vibrant flat colors"},
};

const std::size_t kCameraCount = sizeof(kCameras) / sizeof(kCameras[0]);
const std::size_t kStyleCount = sizeof(kStyles) / sizeof(kStyles[0]);

const Camera* find_camera(const char* id) {
    if (!id) return nullptr;
    for (const Camera& c : kCameras) {
        if (std::strcmp(c.id, id) == 0) return &c;
    }
    return nullptr;
}

const Style* find_style(const char* id) {
    if (!id) return nullptr;
    for (const Style& s : kStyles) {
        if (std::strcmp(s.id, id) == 0) return &s;
    }
    return nullptr;
}

// Trims surrounding whitespace and trailing periods so appended phrases read cleanly.
std::string trim_prompt(const std::string& in) {
    std::size_t b = 0;
    std::size_t e = in.size();
    while (b < e && std::isspace(static_cast<unsigned char>(in[b]))) ++b;
    while (e > b && (std::isspace(static_cast<unsigned char>(in[e - 1])) || in[e - 1] == '.')) --e;
    return in.substr(b, e - b);
}

std::string json_string(const char* s) {
    std::string out = "\"";
    for (const char* p = s; *p; ++p) {
        switch (*p) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            default: out += *p; break;
        }
    }
    out += "\"";
    return out;
}

char* to_c_string(const std::string& s) {
    char* p = static_cast<char*>(std::malloc(s.size() + 1));
    if (!p) return nullptr;
    std::memcpy(p, s.c_str(), s.size() + 1);
    return p;
}

}  // namespace

// No C++ exception may cross the extern "C" boundary (that is undefined behavior for the Rust
// caller). Every entry point catches everything and returns NULL; the Rust side treats NULL as
// "engine unavailable" and falls back to the unmodified prompt.

extern "C" const char* pc_compose(const char* prompt, const char* camera_id, const char* style_id) {
    try {
        const std::string original = prompt ? prompt : "";
        const Camera* cam = find_camera(camera_id);
        const Style* sty = find_style(style_id);
        if (!cam && !sty) return to_c_string(original);

        std::string out = trim_prompt(original);
        if (cam) out += std::string(". ") + cam->phrase;
        if (sty) out += std::string(". ") + sty->phrase;
        out += ".";
        return to_c_string(out);
    } catch (...) {
        return nullptr;
    }
}

extern "C" const char* pc_presets_json(void) {
    try {
        std::string out = "{\"camera\":[";
        for (std::size_t i = 0; i < kCameraCount; ++i) {
            const Camera& c = kCameras[i];
            if (i) out += ",";
            out += "{\"id\":" + json_string(c.id) + ",\"name\":" + json_string(c.name) +
                   ",\"group\":" + json_string(c.group) + "}";
        }
        out += "],\"styles\":[";
        for (std::size_t i = 0; i < kStyleCount; ++i) {
            const Style& s = kStyles[i];
            if (i) out += ",";
            out += "{\"id\":" + json_string(s.id) + ",\"name\":" + json_string(s.name) + "}";
        }
        out += "]}";
        return to_c_string(out);
    } catch (...) {
        return nullptr;
    }
}

extern "C" void pc_free(const char* p) {
    std::free(const_cast<char*>(p));
}
