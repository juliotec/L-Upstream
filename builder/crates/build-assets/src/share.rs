//! Save my pipeline list: the zip a player attaches to the "Submit your pipelines" issue (#14), so that the pipelines
//! their play built go into the list the next release builds before the game ([`crate::pipelines`]).
//!
//! The zip holds two files and nothing else: the game folder's `longshot_pipeline_states.bin` (in its `cache/`;
//! checked first, as a list someone sent is: whole, of a format this build reads, holding pipelines) and
//! `longshot_pipeline_report.txt`, the issue's template filled in from the game folder: the version, the OS, the GPU
//! and its driver (the game's last log), the output and render resolution, the upscaler, frame generation and HDR
//! (`longshot_options.ini`: the one the log names in the user folder, else the game folder's) and the mods in its
//! mods folder. Where the player played is left for them to fill in. Nothing is uploaded.
//!
//! The game's Insert menu does the same in C++ (`gpu/vk/pipeline_share.h`); this is the builders'.

use crate::pipelines;
use build_core::{Error, IoContext, Result, fsx};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The issue the zip goes to.
pub const ISSUE_URL: &str = "https://git.magicbean.dev/MagicBeanDev/Longshot/issues/14";
/// The list, in the shader cache folder (the game folder's `cache/`, or the shared one: gpu/vk/vk_paths.h cache_path).
pub const LIST: &str = "longshot_pipeline_states.bin";
/// The report, in the zip.
pub const REPORT: &str = "longshot_pipeline_report.txt";
/// The issue's template, its lines in its order (`gpu/vk/pipeline_share.h` writes the same).
pub const FIELDS: [&str; 8] = [
    "Longshot version",
    "OS, GPU and driver",
    "Output resolution",
    "Upscaler + render resolution",
    "Frame generation / HDR",
    "Mods or texture packs",
    "Where you played (areas, dungeons, age, night/rain, ...)",
    "Pipelines",
];
const UPSCALERS: [&str; 9] = [
    "Game's own",
    "NVIDIA DLSS",
    "NVIDIA DLAA",
    "AMD FSR 3.1",
    "AMD FSR 4",
    "AMD FSR 4 Native AA",
    "Off (no upscaling)",
    "Intel XeSS",
    "Intel XeSS Native AA",
];
const PERCENT: [u32; 10] = [0, 100, 75, 67, 50, 58, 33, 42, 83, 92];
const FRAMEGEN: [&str; 9] = ["Off", "Auto", "2x", "3x", "4x", "5x", "6x", "7x", "8x"];
const HDR: [&str; 3] = ["Off", "Expanded", "Native (the game's)"];
const OUTPUTS: [(u32, u32); 6] = [(2048, 1152), (2560, 1440), (3840, 2160), (1920, 1080), (5120, 1440), (5120, 2880)];
const HANDHELD: (u32, u32) = (1536, 864);

/// Why there is nothing to share, said to the player as it is.
fn share_error(text: String) -> Error {
    Error::player("pipelines_not_shared", text)
}

/// The list's bytes as read, once, and its records: checked as a list someone sent is (whole records), so the zip
/// holds exactly what was checked.
pub fn read_list(path: &Path) -> Result<(Vec<u8>, Vec<Vec<u8>>)> {
    let data = std::fs::read(path).map_err(|_| {
        share_error(format!(
            "No pipeline list yet: {LIST} is not in the game folder. The game writes it as you play: play a while, then save it again."
        ))
    })?;
    // (no more than a list someone sent may be: an xz the game folder holds could unpack to any size)
    let body = pipelines::unpack(&data, LIST, Some(pipelines::SENT_MOST))
        .map_err(|e| share_error(format!("{LIST} cannot be shared: {e}.")))?;
    let recs: Vec<Vec<u8>> = match pipelines::parse(&body, LIST, true) {
        Ok(r) => r.into_iter().map(<[u8]>::to_vec).collect(),
        Err(e) => {
            let why = e.to_string().replace(&format!("{LIST}: "), "");
            let why = if why.starts_with("not a pipeline state list") {
                "it is not a pipeline state list this build reads".to_string()
            } else if why.starts_with("cut short") {
                format!("{why}. Delete it, play again, then save it again")
            } else {
                why
            };
            return Err(share_error(format!("{LIST} cannot be shared: {}.", why.trim_end_matches('.'))));
        }
    };
    if recs.is_empty() {
        return Err(share_error(format!("{LIST} holds no pipelines yet. Play a while, then save it again.")));
    }
    Ok((data, recs))
}

/// The settings file the game played with: the one its log names, else the game folder's.
fn options_file(folder: &Path, log: &[String]) -> PathBuf {
    for line in log.iter().rev() {
        if let Some(i) = line.find("options: the settings file is ") {
            let rest = line[i + "options: the settings file is ".len()..].trim_end();
            if rest.ends_with("longshot_options.ini") {
                return PathBuf::from(rest);
            }
        }
    }
    folder.join("longshot_options.ini")
}

fn read_ini(folder: &Path, log: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(bytes) = std::fs::read(options_file(folder, log)) {
        for line in String::from_utf8_lossy(&bytes).lines() {
            if let Some((k, v)) = line.trim().split_once('=')
                && !k.trim().is_empty()
            {
                out.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    out
}

fn int(values: &BTreeMap<String, String>, key: &str) -> i64 {
    values.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The game's output as the options set it: (width, height, words).
fn output_size(values: &BTreeMap<String, String>) -> (u32, u32, &'static str) {
    if int(values, "handheld_profile") != 0 {
        return (HANDHELD.0, HANDHELD.1, "Handheld profile");
    }
    let own = |(w, h): (u32, u32)| (w, h, if (w, h) == OUTPUTS[0] { "the game's own" } else { "" });
    let text = values.get("output_res").map_or("", String::as_str).trim_start();
    let digits = |s: &str| s.chars().take_while(char::is_ascii_digit).collect::<String>();
    let w = digits(text);
    if !w.is_empty() {
        let rest = text[w.len()..].trim_start();
        if let Some(rest) = rest.strip_prefix(['x', 'X']) {
            let h = digits(rest.trim_start());
            if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                let (w, h) = (w & !7, h & !7);
                if w >= 640 && h >= 360 && w * h <= 5120 * 2880 && 4 * w >= 5 * h && 9 * w <= 48 * h {
                    return own((w as u32, h as u32));
                }
                return own(OUTPUTS[0]);
            }
        }
    }
    let v = int(values, "output_res");
    own(if (0..OUTPUTS.len() as i64).contains(&v) { OUTPUTS[v as usize] } else { OUTPUTS[0] })
}

fn output_text(values: &BTreeMap<String, String>) -> String {
    let (w, h, words) = output_size(values);
    let mut extra: Vec<&str> = if words.is_empty() { vec![] } else { vec![words] };
    if int(values, "ultrawide") != 0 && words != "Handheld profile" {
        extra.push("Ultrawide on");
    }
    if extra.is_empty() { format!("{w}x{h}") } else { format!("{w}x{h} ({})", extra.join(", ")) }
}

fn upscaler_text(values: &BTreeMap<String, String>) -> String {
    let up = int(values, "dlss");
    let up = if (0..UPSCALERS.len() as i64).contains(&up) { up as usize } else { 0 };
    let name = UPSCALERS[up];
    if up == 2 || up == 5 {
        return format!("{name}, Native");
    }
    let v = int(values, "resolution");
    let p = if v > 0 && (v as usize) < PERCENT.len() { PERCENT[v as usize] } else { 0 };
    if p == 0 {
        return format!("{name}, Auto (dynamic)");
    }
    let (w, h, _) = output_size(values);
    let k = ((f64::from(p) / 100.0 * 12.0 + 0.5) as u32).clamp(1, 12); // (a twelfth of the output: vk_menu.cpp game_size)
    let size = format!("{}x{}", (w * k).div_ceil(12), h * k / 12);
    let mode = match p {
        67 => Some("Quality"),
        58 => Some("Balanced"),
        50 => Some("Performance"),
        33 => Some("Ultra Performance"),
        _ => None,
    }
    .filter(|_| [1, 3, 4].contains(&up));
    format!("{name}, {}{p}% ({size})", mode.map(|m| format!("{m} ")).unwrap_or_default())
}

fn pick<'a>(list: &[&'a str], v: i64) -> &'a str {
    if (0..list.len() as i64).contains(&v) { list[v as usize] } else { "Off" }
}

fn framegen_hdr_text(values: &BTreeMap<String, String>) -> String {
    format!("{} / {}", pick(&FRAMEGEN, int(values, "frame_generation")), pick(&HDR, int(values, "hdr")))
}

/// The mods the game loads from the game folder's mods folder, by `game/mods.c`'s rule: a folder holding `romfs` or
/// `exefs`, up to four folders deep, those whose names start with `-` or `_` left out (off).
pub fn find_mods(folder: &Path) -> Vec<String> {
    fn walk(d: &Path, name: &str, depth: usize, found: &mut Vec<String>) {
        if depth > 0 && (d.join("romfs").is_dir() || d.join("exefs").is_dir()) {
            found.push(name.to_string());
            return;
        }
        if depth >= 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(d) else { return };
        let mut names: Vec<String> =
            entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort_by_key(|n| n.to_lowercase());
        for e in names {
            if e.starts_with(['.', '-', '_']) || !d.join(&e).is_dir() {
                continue;
            }
            walk(&d.join(&e), &if name.is_empty() { e.clone() } else { format!("{name}/{e}") }, depth + 1, found);
        }
    }
    let mut found = Vec::new();
    walk(&folder.join("mods"), "", 0, &mut found);
    found.sort_by_key(|n| n.to_lowercase());
    found
}

fn log_lines(folder: &Path) -> Vec<String> {
    for d in [folder.join("logs"), folder.to_path_buf(), folder.join("bin")] {
        if let Ok(mut f) = std::fs::File::open(d.join("longshot_log.txt")) {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(&mut f, 4 << 20), &mut buf);
            return String::from_utf8_lossy(&buf).lines().map(str::to_string).collect();
        }
    }
    Vec::new()
}

/// The version the game's last run said (its log's first lines), else `longshot_version.txt`'s.
fn version_text(folder: &Path, log: &[String]) -> String {
    for line in log.iter().take(5) {
        if let Some(i) = line.find("longshot ")
            && (i == 0 || !line.as_bytes()[i - 1].is_ascii_alphanumeric())
        {
            let rest = &line[i + 9..];
            let word: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if word.is_empty() {
                continue;
            }
            let after = &rest[word.len()..];
            if let Some(src) = after.strip_prefix(" (source ")
                && let Some(end) = src.find(')')
                && src[..end].chars().all(|c| c.is_ascii_hexdigit())
                && end > 0
            {
                return format!("{word} (source {})", &src[..end]);
            }
            return word;
        }
    }
    std::fs::read_to_string(folder.join("longshot_version.txt"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("version=").map(|v| v.trim().to_string())))
        .unwrap_or_default()
}

/// A Vulkan driver version as its vendor numbers it (`name`: the GPU's or its driver's, which says the vendor).
pub fn driver_number(name: &str, v: u64) -> String {
    let n = name.to_lowercase();
    if n.contains("nvidia") {
        format!("{}.{}.{}", v >> 22, v >> 14 & 0xFF, v >> 6 & 0xFF)
    } else if n.contains("intel") && cfg!(windows) {
        format!("{}.{}", v >> 14, v & 0x3FFF)
    } else {
        format!("{}.{}.{}", v >> 22, v >> 12 & 0x3FF, v & 0xFFF)
    }
}

/// The GPU the game's last run used and its driver, from its log.
fn gpu_text(log: &[String]) -> String {
    let (mut gpu, mut number, mut driver) = (String::new(), None, String::new());
    for line in log {
        if let Some(i) = line.find("vk: ")
            && let Some(j) = line[i..].find(", Vulkan ")
        {
            let rest = &line[i + j + 9..];
            if let Some(k) = rest.find(", driver ")
                && let Some(end) = rest[k + 9..].find("; mesh shaders")
            {
                let raw = &rest[k + 9..k + 9 + end];
                let parsed =
                    raw.strip_prefix("0x").map_or_else(|| raw.parse().ok(), |h| u64::from_str_radix(h, 16).ok());
                if let Some(n) = parsed {
                    gpu = line[i + 4..i + j].to_string();
                    number = Some(n);
                }
            }
        }
        if let Some(i) = line.find("vk: driver: ") {
            driver = line[i + 12..].trim().to_string();
        }
    }
    if gpu.is_empty() {
        return String::new();
    }
    if driver.is_empty()
        && let Some(n) = number
    {
        driver = driver_number(&gpu, n);
    }
    if driver.is_empty() { gpu } else { format!("{gpu}, driver {driver}") }
}

fn os_text() -> String {
    if cfg!(windows) {
        return "Windows".into();
    }
    let pretty = ["/etc/os-release", "/usr/lib/os-release"].iter().find_map(|p| {
        std::fs::read_to_string(p)
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim().trim_matches('"').to_string()))
    });
    let kernel =
        std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()).unwrap_or_default();
    match pretty {
        Some(p) => format!("Linux ({p}, kernel {kernel})"),
        None => format!("Linux (kernel {kernel})"),
    }
}

/// The Pipelines line: the count and the list's version, and how many are compute programs.
fn pipelines_text(recs: &[Vec<u8>]) -> String {
    let computes = recs.iter().filter(|r| pipelines::kind(r) == pipelines::COMPUTE).count();
    let version = if recs.first().is_some_and(|r| r.len() == pipelines::SIZE2) { "version 2" } else { "version 1" };
    let more = if computes > 0 { format!(", {computes} of them compute programs") } else { String::new() };
    format!("{} ({version}{more})", recs.len())
}

/// The issue's template, filled in: `- <field>: <value>` a line.
pub fn template(values: &[String]) -> String {
    FIELDS
        .iter()
        .zip(values)
        .map(|(k, v)| if v.is_empty() { format!("- {k}:\n") } else { format!("- {k}: {v}\n") })
        .collect()
}

/// The template's values, in [`FIELDS`]' order (empty for one to fill in).
pub fn details(folder: &Path, recs: &[Vec<u8>]) -> Vec<String> {
    let log = log_lines(folder);
    let values = read_ini(folder, &log);
    let gpu = gpu_text(&log);
    let mods = find_mods(folder);
    vec![
        version_text(folder, &log),
        format!("{}; {}", os_text(), if gpu.is_empty() { "(GPU and driver: fill in)".into() } else { gpu }),
        output_text(&values),
        upscaler_text(&values),
        framegen_hdr_text(&values),
        if mods.is_empty() { "none".into() } else { mods.join(", ") },
        String::new(),
        pipelines_text(recs),
    ]
}

/// Today's date (UTC), `YYYY-MM-DD`.
fn today() -> String {
    let days =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // (the civil date of a day number: Howard Hinnant's days_from_civil, inverted)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Where a browser's file picker opens: the Downloads folder (on Linux the one `user-dirs.dirs` names), else home.
pub fn downloads_dir() -> PathBuf {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    if !cfg!(windows) {
        let conf = std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join("user-dirs.dirs");
        if let Ok(text) = std::fs::read_to_string(conf) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                    let d = PathBuf::from(v.trim().trim_matches('"').replace("$HOME", &home.to_string_lossy()));
                    if d.is_absolute() && d != home && d.is_dir() {
                        return d;
                    }
                }
            }
        }
    }
    let d = home.join("Downloads");
    if d.is_dir() { d } else { home }
}

/// The game folder's pipeline list: in its `cache/`, or loose in the folder from a build before `cache/`.
pub fn list_path(folder: &Path) -> PathBuf {
    let cached = folder.join("cache").join(LIST);
    if cached.is_file() { cached } else { folder.join(LIST) }
}

/// A zip of `files` (name, bytes), deflated.
fn zip(files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).map_err(|err| Error::format(REPORT, err.to_string()))?;
        let packed = e.finish().map_err(|err| Error::format(REPORT, err.to_string()))?;
        let crc = crc32fast::hash(data);
        let at = out.len() as u32;
        let head = |sig: u32, central: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if central {
                h.extend(20u16.to_le_bytes()); // (made by)
            }
            for v in [20u16, 0, 8, 0, 0x21] {
                h.extend(v.to_le_bytes()); // (version needed, flags, deflate, time, date: 1980-01-01)
            }
            h.extend(crc.to_le_bytes());
            h.extend((packed.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 6]); // (comment, disk, internal attributes)
                h.extend(0u32.to_le_bytes());
                h.extend(at.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(head(0x0403_4B50, false));
        out.extend(&packed);
        central.extend(head(0x0201_4B50, true));
    }
    let (cd_at, cd_len) = (out.len() as u32, central.len() as u32);
    out.extend(central);
    out.extend(0x0605_4B50u32.to_le_bytes());
    out.extend([0u8; 4]);
    for _ in 0..2 {
        out.extend((files.len() as u16).to_le_bytes());
    }
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

/// What [`make_zip`] made.
#[derive(Debug)]
pub struct Shared {
    /// The zip.
    pub zip: PathBuf,
    /// The filled template, to paste into the issue.
    pub template: String,
    /// The pipelines in it.
    pub pipelines: usize,
}

/// The zip for the issue, `longshot-pipelines-<date>.zip` in `out_dir` (default: the Downloads folder), from the game
/// folder `folder`. A player error when the list is missing or not one to share (nothing written then).
pub fn make_zip(folder: &Path, out_dir: Option<&Path>) -> Result<Shared> {
    make_zip_in(folder, None, out_dir)
}

/// [`make_zip`], the list read from the shader cache folder `cache` when it has one (the shared one play's game keeps
/// its caches in), else from the game folder ([`list_path`]).
pub fn make_zip_in(folder: &Path, cache: Option<&Path>, out_dir: Option<&Path>) -> Result<Shared> {
    let list = cache.map(|c| c.join(LIST)).filter(|l| l.is_file()).unwrap_or_else(|| list_path(folder));
    let (data, recs) = read_list(&list)?;
    let text = template(&details(folder, &recs));
    let note = format!(
        "Attach this zip to a comment on {ISSUE_URL} and paste the lines below into it, with where you played filled in.\n\
         The zip holds only {LIST} (shader hashes, formats and draw state, and the hashes of the compute programs your \
         play ran: no game code, game data or personal info) and this text.\n"
    );
    let report = format!("{note}\n{text}");
    let dir = out_dir.map_or_else(downloads_dir, Path::to_path_buf);
    std::fs::create_dir_all(&dir).at(&dir)?;
    let out = dir.join(format!("longshot-pipelines-{}.zip", today()));
    fsx::write_atomic(&out, &zip(&[(LIST, &data), (REPORT, report.as_bytes())])?)?;
    Ok(Shared { zip: out, template: text, pipelines: recs.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ini(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_settings_as_the_menu_names_them() {
        assert_eq!(output_text(&ini(&[])), "2048x1152 (the game's own)");
        assert_eq!(output_text(&ini(&[("output_res", "2560x1440"), ("ultrawide", "1")])), "2560x1440 (Ultrawide on)");
        assert_eq!(output_text(&ini(&[("handheld_profile", "1")])), "1536x864 (Handheld profile)");
        assert_eq!(
            upscaler_text(&ini(&[("dlss", "1"), ("resolution", "3"), ("output_res", "2")])),
            "NVIDIA DLSS, Quality 67% (2560x1440)"
        );
        assert_eq!(upscaler_text(&ini(&[("dlss", "2")])), "NVIDIA DLAA, Native");
        assert_eq!(upscaler_text(&ini(&[])), "Game's own, Auto (dynamic)");
        assert_eq!(framegen_hdr_text(&ini(&[("frame_generation", "2"), ("hdr", "9")])), "2x / Off");
    }

    #[test]
    fn the_log_s_gpu_and_version() {
        let log: Vec<String> = [
            "longshot v3.1.0 (source 1a2b3c) starting",
            "vk: NVIDIA GeForce RTX 3090, Vulkan 1.3, driver 0x9b0a0000; mesh shaders on",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(version_text(Path::new("/nowhere"), &log), "v3.1.0 (source 1a2b3c)");
        assert_eq!(gpu_text(&log), "NVIDIA GeForce RTX 3090, driver 620.40.0");
        assert_eq!(template(&["a".into(), String::new()]), "- Longshot version: a\n- OS, GPU and driver:\n");
    }

    #[test]
    fn a_zip_of_the_list_and_the_report() {
        let d = std::env::temp_dir().join(format!("build-assets-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("cache")).unwrap();
        assert!(matches!(make_zip(&d, Some(&d)), Err(Error::Player { code: "pipelines_not_shared", .. })));
        let (list, _) = pipelines::pack(vec![pipelines::compute_record(9)]);
        std::fs::write(d.join("cache").join(LIST), &list).unwrap();
        let s = make_zip(&d, Some(&d)).unwrap();
        assert_eq!(s.pipelines, 1);
        assert!(s.template.contains("- Pipelines: 1 (version 2, 1 of them compute programs)"));
        let z = std::fs::read(&s.zip).unwrap();
        assert!(z.starts_with(b"PK\x03\x04"));
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        // (each central record where the one before ends, the last one at the end record)
        let mut at = u32::from_le_bytes(z[z.len() - 6..z.len() - 2].try_into().unwrap()) as usize;
        for _ in 0..2 {
            assert_eq!(&z[at..at + 4], b"PK\x01\x02");
            let name = u16::from_le_bytes([z[at + 28], z[at + 29]]) as usize;
            at += 46 + name;
        }
        assert_eq!(at, z.len() - 22);
        assert_eq!(today().len(), 10);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
