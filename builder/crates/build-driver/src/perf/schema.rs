//! What a summary may hold, as the game writes it (`gpu/vk/perf_report.h`'s `summary_json`, its histograms' bucket
//! counts from `gpu/vk/perf_report_schema.h`): a file in `perf_pending/` that is anything else is not sent. The game
//! folder is the player's to write; this keeps what goes to the relay to what the game would have sent.

use serde_json::Value;

/// The summary's own keys.
pub const TOP: [&str; 24] = [
    "v",
    "client",
    "version",
    "release",
    "first",
    "session_s",
    "os",
    "kernel",
    "renderer",
    "gpu",
    "driver",
    "vram_gb",
    "cpu",
    "ram_gb",
    "upscaler",
    "preset",
    "fps_cap",
    "present_mode",
    "vsync",
    "framegen",
    "map",
    "memory",
    "maps",
    "samples",
];
/// `memory`'s.
pub const MEMORY: [&str; 3] = ["vram_used_gb", "vram_allowed_gb", "ram_used_gb"];
/// Each map's totals' keys.
pub const MAP: [&str; 17] = [
    "new",
    "seconds",
    "frame_ms",
    "fine_ms",
    "frame_ms_sum",
    "gpu_ms",
    "gpu_ms_sum",
    "parts_ms",
    "phases_ms",
    "hitches",
    "pacing_misses",
    "draws_left_out",
    "shown_ms",
    "waits_ms",
    "causes",
    "events",
    "movie",
];
/// The map totals' objects of fixed keys.
pub const FIXED: [(&str, &[&str]); 4] = [
    ("parts_ms", &["game", "render", "gpu_wait", "pace"]),
    ("shown_ms", &["sum", "count"]),
    (
        "causes",
        &[
            "main_cpu_ms",
            "main_blocked_ms",
            "main_vcore_ms",
            "late_wait_ms",
            "tex_frame_ms",
            "upload_mb",
            "uploads",
            "pipe_waits",
            "textures_gone",
        ],
    ),
    ("movie", &["shown", "dropped", "late", "decode_ms", "fallbacks"]),
];
/// The map totals' objects named by the game's own tables (phases, hitch causes, waits, events): numbers by name.
pub const NAMED: [&str; 4] = ["phases_ms", "hitches", "waits_ms", "events"];
/// A position sample's keys.
pub const SAMPLE: [&str; 11] =
    ["map", "x", "y", "z", "yaw", "pitch", "frames", "frame_ms", "gpu_frames", "gpu_ms", "hitches"];
/// The histograms and their bucket counts (perf_report_schema.h's `k*BucketCount`: its bounds and +Inf).
pub const HISTOGRAMS: [(&str, usize); 3] = [("frame_ms", 13), ("fine_ms", 19), ("gpu_ms", 11)];

fn scalar(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_))
}

fn only(v: &Value, keys: &[&str], each: impl Fn(&str, &Value) -> bool) -> bool {
    v.as_object().is_some_and(|o| o.iter().all(|(k, x)| keys.contains(&k.as_str()) && each(k, x)))
}

fn numbers(v: &Value) -> bool {
    v.as_object().is_some_and(|o| o.values().all(Value::is_number))
}

fn map_totals(v: &Value) -> bool {
    only(v, &MAP, |k, x| {
        if let Some(&(_, n)) = HISTOGRAMS.iter().find(|h| h.0 == k) {
            return x.as_array().is_some_and(|a| a.len() == n && a.iter().all(Value::is_number));
        }
        if let Some((_, keys)) = FIXED.iter().find(|f| f.0 == k) {
            return only(x, keys, |_, y| y.is_number());
        }
        if NAMED.contains(&k) {
            return numbers(x);
        }
        scalar(x)
    })
}

/// Whether `v` is a summary as the game writes one: its keys and their kinds, each map's histograms of the schema's
/// bucket counts, no key or nesting of anything else.
pub fn fits(v: &Value) -> bool {
    only(v, &TOP, |k, x| match k {
        "memory" => only(x, &MEMORY, |_, y| y.is_number()),
        "maps" => x.as_object().is_some_and(|m| m.values().all(map_totals)),
        "samples" => x.as_array().is_some_and(|s| s.iter().all(|e| only(e, &SAMPLE, |_, y| scalar(y)))),
        _ => scalar(x),
    }) && v.get("v").is_some_and(Value::is_number)
}
