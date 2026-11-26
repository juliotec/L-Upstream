//! What the run's log and its record say, and what the window says from them: whether it was a crash
//! ([`verdict`]), when ([`Stage`]) and why where that is known ([`Cause`]). Ported from host/hos_crash_dialog.h.

/// The names of the time before the first map (the crash report's and the performance summary's).
pub const TITLE_AND_INTRO: &str = "Title and intro";
/// The shaders' preparation, as the report's map says it.
pub const PREPARING_SHADERS: &str = "Preparing shaders";

/// `from`'s first line, at most `cap - 1` bytes (never a character cut in two): the C's fixed fields.
pub(crate) fn take(from: &str, cap: usize) -> String {
    let line = &from[..from.find(['\n', '\r']).unwrap_or(from.len())];
    cut(line, cap.saturating_sub(1)).to_string()
}

/// `s`, at most `n` bytes, on a character's boundary.
pub(crate) fn cut(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Why Vulkan listed no GPU (gpu/vk/compat/longshot_no_gpu.h's causes, by their names in the renderer's line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoGpu {
    /// Nothing more known.
    Unknown,
    /// An NVIDIA card, NVIDIA's driver not loaded, a real-time kernel.
    NvidiaRt,
    /// An NVIDIA card, NVIDIA's driver not loaded.
    NvidiaUnloaded,
    /// NVIDIA's driver loaded, but no Vulkan device listed.
    NvidiaVulkan,
}

impl NoGpu {
    fn of(key: &str) -> NoGpu {
        match key {
            "nvidia-rt" => NoGpu::NvidiaRt,
            "nvidia-unloaded" => NoGpu::NvidiaUnloaded,
            "nvidia-vulkan" => NoGpu::NvidiaVulkan,
            _ => NoGpu::Unknown,
        }
    }
}

/// What the run's log says, line by line ([`LogFacts::line`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFacts {
    /// A map line (host/hos_main.c: a save loaded).
    pub map: bool,
    /// The shaders' preparation began.
    pub preparing: bool,
    /// The shaders' preparation ended.
    pub prepared: bool,
    /// The game's files are missing or don't match.
    pub files: bool,
    /// The Vulkan back end did not start.
    pub vulkan: bool,
    /// The window did not open.
    pub window: bool,
    /// The guest's window could not be reserved.
    pub reserve: bool,
    /// Out of memory.
    pub memory: bool,
    /// The start warned of low memory.
    pub low_memory: bool,
    /// The device was lost.
    pub device_lost: bool,
    /// Vulkan listed no GPU at all, and why.
    pub no_gpu: Option<NoGpu>,
    /// The log's version line.
    pub version: String,
    /// The device chosen.
    pub gpu: String,
    /// Its driver.
    pub driver: String,
    /// Every device listed.
    pub devices: String,
    /// DLSS's first line.
    pub dlss: String,
    /// FSR 4's first line.
    pub fsr4: String,
    /// `report: os`.
    pub os: String,
    /// `report: cpu`.
    pub cpu: String,
    /// `report: renderer`.
    pub renderer: String,
    /// The place: the last map, else the shaders' preparation or the title and intro.
    pub place: String,
    /// The crash in one line ([`summary`]).
    pub crash: String,
    /// The renderer's own `report: gpu` line.
    pub gpu_report: String,
}

/// The crash in one line, as the report's summary says it: "0xc0000005 (write) at longshot+0x1a5b9928" from
/// "CRASH: exception 0xc0000005 (write 0x10, guest 0x...) at /path/to/longshot+0x1a5b9928"; "" when it is not one.
pub fn summary(line: &str) -> String {
    let Some(at) = line.find("CRASH: exception 0x") else { return String::new() };
    let code = &line[at + 17..];
    let b = code.as_bytes();
    let mut nc = 2;
    while nc < b.len() && b[nc].is_ascii_hexdigit() {
        nc += 1;
    }
    let how = &code[nc..];
    if !how.starts_with(" (read ") && !how.starts_with(" (write ") {
        return String::new();
    }
    let kind = if how.as_bytes()[2] == b'r' { "read" } else { "write" };
    let Some(close) = how.find(')') else { return String::new() };
    if !how[close..].starts_with(") at ") {
        return String::new();
    }
    let place = &how[close + 5..];
    let np = place.find(|c: char| c.is_ascii_whitespace()).unwrap_or(place.len());
    let place = &place[..np];
    let plus = place.rfind("+0x").filter(|&i| i + 2 < place.len());
    let name = match plus {
        Some(p) => place[..p].rfind(['/', '\\']).map_or(place, |s| &place[s + 1..]),
        None => place,
    };
    cut(&format!("{} ({kind}) at {name}", &code[..nc]), 199).to_string()
}

/// The log's first line's version, "v3.0.0 (source 47395392)": the version alone, and its source fingerprint ("" none).
pub fn version_split(line: &str) -> (String, String) {
    let Some(at) = line.find(" (source ") else { return (line.to_string(), String::new()) };
    let s = &line[at + 9..];
    let k = s.bytes().take_while(u8::is_ascii_hexdigit).count();
    let source = if s.as_bytes().get(k) == Some(&b')') { s[..k].to_string() } else { String::new() };
    (line[..at].to_string(), source)
}

fn digits(s: &[u8], p: &mut usize) -> bool {
    let from = *p;
    while *p < s.len() && s[*p].is_ascii_digit() {
        *p += 1;
    }
    *p > from
}

/// A release's version (the report's release flag): v1.2.3, v1.2.3-rc.4, nightly-<count>-<8 hex>.
pub fn release_version(v: &str) -> bool {
    let s = v.as_bytes();
    let mut p = 0;
    if let Some(rest) = v.strip_prefix("nightly-") {
        let r = rest.as_bytes();
        if !digits(r, &mut p) || r.get(p) != Some(&b'-') {
            return false;
        }
        let hex = &r[p + 1..];
        return hex.len() == 8 && hex.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
    }
    if s.first() == Some(&b'v') {
        p = 1;
    }
    for part in 0..3 {
        if !digits(s, &mut p) {
            return false;
        }
        if part < 2 {
            if s.get(p) != Some(&b'.') {
                return false;
            }
            p += 1;
        }
    }
    if s[p..].starts_with(b"-rc.") {
        p += 4;
        if !digits(s, &mut p) {
            return false;
        }
    }
    p == s.len()
}

/// A release build, as the report says it: a release's version, built from the release's own sources (the log's
/// source fingerprint is the one longshot_version.txt's `source=` names).
pub fn release(version: &str, source: &str, stamp_source: &str) -> bool {
    release_version(version) && !source.is_empty() && source == stamp_source
}

impl LogFacts {
    /// A line of the log (already without its home folder: [`crate::redact`]).
    pub fn line(&mut self, line: &str) {
        let has = |s: &str| line.contains(s);
        let after = |s: &str| line.find(s).map(|i| &line[i + s.len()..]);
        if has("] map: ") {
            self.map = true;
        }
        if has("] vk: start-up: the shaders took") {
            self.prepared = true;
        } else if has("] vk: start-up: ")
            || has("] vk: precompile")
            || has("] vk: preparation: ")
            || has("] vk: prepared ")
        {
            self.preparing = true;
        }
        if line.starts_with("cannot open ")
            || line.starts_with("usage: ")
            || has(": lift and image do not match")
            || has("] rx: mkwvk_init version mismatch")
            || has(" has no mkwvk_init")
        {
            self.files = true;
        }
        if line.starts_with("cannot reserve the guest window") {
            self.reserve = true;
        }
        if has("] rx: RX_BACKEND=vulkan, but ")
            || has("] vk: load_global failed")
            || has("] vk: vkCreateInstance failed")
            || has("] vk: no Vulkan 1.3 GPU")
            || has("] vk: create_device failed")
            || has("] rx: the Vulkan back end could not start")
            || has("] display: the renderer did not start")
        {
            self.vulkan = true;
        }
        if let Some(key) = after("] vk: no Vulkan device (cause ") {
            let cause = key.find(')').map_or(NoGpu::Unknown, |end| NoGpu::of(&key[..end]));
            self.vulkan = true;
            self.no_gpu = Some(cause);
        }
        if has("] display: no SDL")
            || has("] display: SDL_Init: ")
            || has("] display: SDL_CreateWindow: ")
            || after("] display: ").is_some_and(|a| a.contains(" failed (error "))
        {
            self.window = true;
        }
        if has("] mem: Windows is out of memory")
            || after("] mem: commit of ").is_some_and(|a| a.contains(" failed (error "))
        {
            self.memory = true;
        }
        if has("] boot: LOW MEMORY") {
            self.low_memory = true;
        }
        if has("device lost") || has("VK_ERROR_DEVICE_LOST") {
            self.device_lost = true;
        }
        if self.version.is_empty()
            && let Some(v) = after("] longshot ")
        {
            self.version = take(v, 128);
        }
        if let Some(v) = after("] vk: driver: ") {
            self.driver = take(v, 160);
        }
        if let Some(a) = after("] vk: ")
            && let Some(end) = a.find(", Vulkan ")
            && a.contains("; mesh shaders ")
        {
            self.gpu = cut(&a[..end], 159).to_string();
        }
        if let Some(a) = after("] vk: device ")
            && a.contains(", Vulkan ")
            && let Some(colon) = a.find(':')
            && let Some(end) = a[colon..].find(", Vulkan ").map(|e| e + colon)
            && end > colon + 2
            && self.devices.len() + (end - colon) + 2 < 240
        {
            if !self.devices.is_empty() {
                self.devices.push_str("; ");
            }
            self.devices.push_str(&a[colon + 2..end]);
        }
        if let Some(v) = after("] report: gpu ") {
            self.gpu_report = take(v, 328);
        }
        if let Some(v) = after("] report: os ") {
            self.os = take(v, 200);
        }
        if let Some(v) = after("] report: cpu ") {
            self.cpu = take(v, 200);
        }
        if let Some(v) = after("] report: renderer ") {
            self.renderer = take(v, 120);
        }
        if self.crash.is_empty()
            && let Some(i) = line.find("CRASH: exception 0x")
        {
            self.crash = summary(&line[i..]);
        }
        if let Some(v) = after("] map: ") {
            self.place = take(v, 120);
        } else if has("] vk: start-up: the shaders took ")
            || has("] vk: preparation: done")
            || has("] vk: shader preparation stopped ")
        {
            self.place = TITLE_AND_INTRO.into();
        } else if (has("] vk: start-up: ")
            || has("] vk: preparation: ")
            || has("] vk: preparing program ")
            || has("] vk: shader preparation "))
            && (self.place.is_empty() || self.place == TITLE_AND_INTRO)
        {
            self.place = PREPARING_SHADERS.into();
        }
        if self.dlss.is_empty()
            && let Some(i) = line.find("] vk: NVIDIA DLSS: ")
        {
            self.dlss = take(&line[i + 6..], 240);
        }
        if self.fsr4.is_empty()
            && let Some(i) = line.find("] vk: AMD FSR 4: ")
        {
            self.fsr4 = take(&line[i + 6..], 240);
        }
    }

    /// Facts of every line of `text`.
    pub fn of(text: &str) -> LogFacts {
        let mut f = LogFacts::default();
        text.lines().for_each(|l| f.line(l));
        f
    }

    /// When it happened.
    pub fn stage(&self) -> Stage {
        if self.map {
            Stage::Play
        } else if self.preparing && !self.prepared {
            Stage::Preparing
        } else {
            Stage::Start
        }
    }

    /// A start that stopped before play for a reason the log names.
    pub fn start_failed(&self) -> bool {
        !self.map && (self.files || self.vulkan || self.window || self.reserve)
    }

    /// The graphics card with its driver, as the report sends it ("NVIDIA GeForce RTX 3090, NVIDIA 580.95.05"): the
    /// renderer's `report: gpu` line, else the device chosen and its driver, else its name alone; "" for none.
    pub fn gpu_sent(&self) -> String {
        if !self.gpu_report.is_empty() {
            self.gpu_report.clone()
        } else if !self.gpu.is_empty() && !self.driver.is_empty() {
            format!("{}, {}", self.gpu, self.driver)
        } else {
            self.gpu.clone()
        }
    }
}

/// When it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// During the start.
    Start,
    /// While it prepared the shaders.
    Preparing,
    /// While the player played.
    Play,
    /// The run's log was not found: nothing is said about when.
    Unknown,
}

impl Stage {
    /// As the window's sentence says it ("" for not known).
    pub fn text(self) -> &'static str {
        match self {
            Stage::Play => "while you were playing",
            Stage::Preparing => "while it prepared the shaders",
            Stage::Unknown => "",
            Stage::Start => "during the start",
        }
    }
}

/// A pending record (hos_crash_record.c's format_record): its `key: value` lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// What it is.
    pub kind: Kind,
    /// (unclean) The player had asked to quit.
    pub quit: bool,
    /// `exception: 0x...`.
    pub code: u32,
    /// `signal:`.
    pub signal: i32,
    /// (unclean) The out-of-memory killer took the game.
    pub oom: bool,
    /// `reason:`.
    pub reason: String,
    /// `module_offset:`.
    pub module: String,
    /// `run_id:`.
    pub run_id: String,
    /// `when:`.
    pub when: String,
    /// `uptime_s:`.
    pub uptime: String,
    /// `pid:`.
    pub pid: u64,
}

/// A record's kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// No record (or not one of ours).
    #[default]
    None,
    /// The game's crash handler wrote it.
    Crash,
    /// The game ended without its crash handler (killed, out of memory).
    Unclean,
}

/// A `key: value` line's value in a record, the first; None without one.
pub fn record_value(text: &str, key: &str) -> Option<String> {
    text.split('\n').find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(": ")).map(|v| take(v, 4096)))
}

impl Record {
    /// A record's text.
    pub fn parse(text: &str) -> Record {
        let kind = if text.starts_with("Longshot crash record 1\n") {
            Kind::Crash
        } else if text.starts_with("Longshot unclean record 1\n") {
            Kind::Unclean
        } else {
            return Record::default();
        };
        let v = |k: &str, cap: usize| record_value(text, k).map(|s| cut(&s, cap - 1).to_string()).unwrap_or_default();
        Record {
            kind,
            reason: v("reason", 40),
            module: v("module_offset", 160),
            run_id: v("run_id", 33),
            when: v("when", 17),
            uptime: v("uptime_s", 24),
            code: record_value(text, "exception")
                .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok())
                .unwrap_or(0),
            signal: record_value(text, "signal").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            quit: record_value(text, "quit_asked").as_deref() == Some("yes"),
            oom: record_value(text, "oom_kill").as_deref() == Some("yes"),
            pid: record_value(text, "pid").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        }
    }
}

/// A crash in the graphics driver's own code: the record's module (a file name, never a path).
pub fn driver_module(module: &str) -> bool {
    const DRIVERS: [&str; 12] = [
        "nvoglv64",
        "nvgpucomp",
        "nvvk",
        "amdvlk",
        "amdxc",
        "igvk",
        "igc",
        "igd",
        "libnvidia-",
        "libvulkan_",
        "libGLX_nvidia",
        "libamdvlk",
    ];
    DRIVERS.iter().any(|d| {
        let m = module.as_bytes();
        m.len() >= d.len()
            && d.bytes().zip(m).all(|(want, &c)| {
                let c = if c.is_ascii_uppercase() && want >= b'a' { c + 32 } else { c };
                c == want
            })
    })
}

/// What the builder says once the game has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing: a clean end, a quit, a refused start, a rebuild asked for.
    Quiet,
    /// It crashed.
    Crash,
    /// It ended without its crash handler (killed, out of memory).
    Unclean,
    /// The start could not go on (its files, Vulkan, the window, memory).
    Start,
}

/// The game's exit status, as the builder saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exit {
    /// Its exit code, where the system gave one (None: a signal ended it).
    pub code: Option<u32>,
}

/// What to say once the game has ended. A normal exit (0) and a quit are not crashes. A crash record of the run: it crashed, whatever came before it, a quit
/// included. A start the log says could not go on before a save was loaded: that, a quit included. An unclean end (the
/// run's marker left with no crash record: killed, out of memory): only when the player had not asked to quit and the
/// exit code (where there is one) is not 0. Anything else says nothing; nor does a run another took the place of.
pub fn verdict(r: &Record, f: &LogFacts, exit: Exit, replaced: bool) -> Verdict {
    if replaced {
        return Verdict::Quiet;
    }
    if r.kind == Kind::Crash {
        return Verdict::Crash;
    }
    if f.start_failed() {
        return Verdict::Start;
    }
    if r.kind == Kind::Unclean && !r.quit && exit.code != Some(0) {
        return Verdict::Unclean;
    }
    Verdict::Quiet
}

/// Why, where the record or the log says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Not known.
    Unknown,
    /// The game's files.
    Files,
    /// Vulkan did not start.
    Vulkan,
    /// The window did not open.
    Window,
    /// Out of memory.
    Memory,
    /// The graphics driver.
    GpuDriver,
    /// Ended from outside.
    Ended,
    /// Vulkan found no GPU.
    NoGpu,
    /// NVIDIA's module, a real-time kernel.
    NvidiaRt,
    /// NVIDIA's module not loaded.
    NvidiaUnloaded,
    /// NVIDIA loaded, no Vulkan device.
    NvidiaVulkan,
}

/// Why: the first that holds of the game's files, Vulkan, the window (before play), memory, the graphics driver; an
/// unclean end with nothing else known was ended from outside (or out of memory, when the start warned of it).
pub fn cause(v: Verdict, r: &Record, f: &LogFacts, exit: Exit) -> Cause {
    if f.files && !f.map {
        return Cause::Files;
    }
    if f.vulkan && !f.map {
        return match f.no_gpu {
            Some(NoGpu::NvidiaRt) => Cause::NvidiaRt,
            Some(NoGpu::NvidiaUnloaded) => Cause::NvidiaUnloaded,
            Some(NoGpu::NvidiaVulkan) => Cause::NvidiaVulkan,
            Some(NoGpu::Unknown) => Cause::NoGpu,
            None => Cause::Vulkan,
        };
    }
    if f.window && !f.map {
        return Cause::Window;
    }
    if f.reserve || f.memory || r.oom || r.code == 0xC000_0017 || exit.code == Some(0xC000_0017) {
        return Cause::Memory;
    }
    if r.reason == "device lost"
        || r.reason == "Vulkan fatal"
        || f.device_lost
        || (r.kind == Kind::Crash && driver_module(&r.module))
    {
        return Cause::GpuDriver;
    }
    if v == Verdict::Unclean {
        return if f.low_memory { Cause::Memory } else { Cause::Ended };
    }
    Cause::Unknown
}

/// The window's title.
pub fn title(v: Verdict) -> &'static str {
    if v == Verdict::Start { "Longshot couldn't start" } else { "Longshot closed unexpectedly" }
}

/// What happened and when, without its full stop: "Longshot closed unexpectedly while you were playing".
pub fn headline(what: &str, stage: Stage) -> String {
    let when = stage.text();
    if when.is_empty() { what.to_string() } else { format!("{what} {when}") }
}

impl Cause {
    /// As the window says it.
    pub fn text(self) -> &'static str {
        match self {
            Cause::Files => {
                "Longshot's game files are missing or don't match each other. Build the game again with the Longshot Launcher."
            }
            Cause::Vulkan => {
                "Vulkan didn't start on your graphics card. Update your graphics driver: Longshot needs a graphics card and \
                 driver with Vulkan 1.3."
            }
            Cause::NoGpu => {
                "Vulkan found no graphics card. Check that a Vulkan driver is installed and loaded, and that Vulkan loader \
                 filters are not hiding your card. Install or update your graphics driver and restart your PC: Longshot needs \
                 a graphics card and driver with Vulkan 1.3."
            }
            Cause::NvidiaRt => {
                "NVIDIA's kernel module isn't loaded and this is a real-time kernel (PREEMPT_RT). A driver compatibility or \
                 loading issue is possible. Check your NVIDIA driver and kernel module logs, and try a supported driver or a \
                 standard kernel. Also check that a Vulkan driver is installed and loaded."
            }
            Cause::NvidiaUnloaded => {
                "NVIDIA's kernel module isn't loaded. This may be a driver loading issue. Restart your PC or reinstall the NVIDIA \
                 driver, and check that a Vulkan driver is installed and loaded."
            }
            Cause::NvidiaVulkan => {
                "NVIDIA's kernel module is loaded, but Vulkan found no graphics card. Check that a Vulkan driver is installed \
                 and loaded. NVIDIA's Vulkan files may be missing or mismatched; try restarting or reinstalling its driver."
            }
            Cause::Window => "Longshot couldn't open its window. Restart your PC, and update your graphics driver.",
            Cause::Memory => {
                "Your PC ran out of memory. Close other programs and try again; Longshot needs 16 GB of RAM, and on Windows a \
                 page file Windows manages."
            }
            Cause::GpuDriver => {
                "The graphics driver failed. Update your graphics driver, and turn off overlays and tools hooked into the game."
            }
            Cause::Ended => {
                "Longshot ended without a crash report: it was closed from outside (Task Manager, a kill), or the system \
                 stopped it, often for running out of memory."
            }
            Cause::Unknown => "An unexpected error.",
        }
    }
}

/// The window's text: what happened and when, why, and what Report manually (and Report automatically, when `send`)
/// does.
pub fn message(v: Verdict, stage: Stage, cause: Cause, send: bool) -> String {
    let headline =
        headline(if v == Verdict::Start { "Longshot stopped" } else { "Longshot closed unexpectedly" }, stage);
    format!(
        "{headline}.\n\nWhat went wrong: {}\n\n{}Report manually copies a report to your clipboard (the error, your PC's hardware \
         and the end of the log, with your home folder and user name taken out) and opens Longshot's issue form in your \
         browser, for you to paste it into. {}",
        cause.text(),
        if send {
            "Report automatically shows you what would go to the Longshot team, and sends it only when you press Send.\n\n"
        } else {
            ""
        },
        if send { "Report manually itself sends nothing." } else { "Longshot itself sends nothing." }
    )
}

#[cfg(test)]
mod tests;
