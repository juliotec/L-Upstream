//! The game's channel to its builder, the builder's side (docs/dev/reference/build.md, "The game's channel to its
//! builder"): JSON lines over the pipe pair `build_core::channel` gives the game it starts.
//!
//! Everything the game sends may be forged (it runs mod code, some of it native), so the channel does only what typed ops
//! say, nothing generic (no fetching a URL, no adding a source), and never grants trust: the player does, in a window the
//! builder owns.
//!
//! ```text
//! game    {"id":1,"op":"hello","v":1}
//! builder {"id":1,"ok":true,"result":{"v":1,"builder":"<stamp>","ops":["hello","profile",...]}}
//! game    {"id":2,"op":"profile","args":{}}
//! builder {"id":2,"ok":true,"result":{"dir":"...","name":"Ann",...}}
//! builder {"id":3,"ok":false,"code":"unknown_op","text":"..."}
//! builder {"event":"profile_changed"}
//! ```
//!
//! The built-in ops: `hello`, `profile`, `set_mods`, `restart_to_apply`, `cancel`. Every other op is the hook's
//! ([`crate::modhook::serve`], which also names them in `hello`'s `ops`). A request that is not one of those, or a name
//! that is none, is `unknown_op`. Rules: a line is at most [`LINE_MAX`] bytes (a longer one closes the channel), at
//! most [`PENDING_MAX`] requests wait for an answer (more is `busy`), `cancel` stops a pending one by its id, the
//! first request is `hello`. When `profile.json` has changed since the last thing said, a `profile_changed` event comes
//! before the next reply.

use crate::modhook::{BuilderCx, Confirmation, Output};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// The protocol's version.
pub const VERSION: u64 = 1;
/// The most bytes of a line (a request or a reply), the newline not counted.
pub const LINE_MAX: usize = 1 << 20;
/// The most requests waiting for their answers at once.
pub const PENDING_MAX: usize = 16;
/// The ops the builder answers itself.
pub const BUILT_IN: [&str; 5] = ["hello", "profile", "set_mods", "restart_to_apply", "cancel"];
/// How long `set_mods` waits for another edit of the profile to finish.
const EDIT_WAIT: Duration = if cfg!(test) { Duration::from_millis(150) } else { Duration::from_secs(2) };

/// A request that reaches the hook ([`crate::modhook::serve`]).
pub struct Request {
    /// Its id: the reply and its events carry it.
    pub id: u64,
    /// The op's name.
    pub op: String,
    /// Its `args` (an object; `{}` when the game sent none).
    pub args: Value,
    /// Set when the game cancelled it (`cancel`): a long op looks at it ([`Request::is_cancelled`]) between steps and
    /// stops with `cancelled`.
    pub cancelled: Arc<AtomicBool>,
}

impl Request {
    /// Whether the request was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// An event, a line `{"event":"<name>",...}` the builder says unasked. `Progress` and `Done` carry the id of the
/// request they are about.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A long op's progress.
    Progress {
        /// The request's id.
        id: u64,
        /// How far, 0 to 1.
        fraction: f64,
        /// What it is doing.
        text: String,
    },
    /// A request has ended (its reply is the result).
    Done {
        /// The request's id.
        id: u64,
    },
    /// `profile.json` has changed since the builder last said anything.
    ProfileChanged,
}

/// Why an op failed: a stable `code` (`unknown_op`, `bad_args`, `refused`, `busy`, `cancelled`, `not_found`,
/// `mod_tooling_missing`, ...) and the sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpError {
    /// Stable id.
    pub code: String,
    /// Why.
    pub text: String,
}

impl OpError {
    /// An error with `code`.
    pub fn new(code: &str, text: impl Into<String>) -> OpError {
        OpError { code: code.into(), text: text.into() }
    }
}

/// A reply: a result, or a `code` and the sentence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reply {
    /// Done, with its result.
    Ok(Value),
    /// Refused or failed.
    Err {
        /// Stable id.
        code: String,
        /// The sentence.
        text: String,
    },
}

impl Reply {
    /// An error reply.
    pub(crate) fn err(code: &str, text: impl Into<String>) -> Reply {
        Reply::Err { code: code.into(), text: text.into() }
    }

    fn of(r: Result<Value, OpError>) -> Reply {
        match r {
            Ok(v) => Reply::Ok(v),
            Err(OpError { code, text }) => Reply::Err { code, text },
        }
    }

    fn line(&self, id: Option<u64>) -> Value {
        let id = id.map_or(Value::Null, Value::from);
        match self {
            Reply::Ok(r) => json!({"id": id, "ok": true, "result": r}),
            Reply::Err { code, text } => json!({"id": id, "ok": false, "code": code, "text": text}),
        }
    }
}

/// What the server knows: the places of a playing profile and who the builder is.
#[derive(Clone)]
pub struct Config {
    /// The profile folder the game plays.
    pub profile: PathBuf,
    /// The version folder.
    pub version: PathBuf,
    /// The builder's cache.
    pub cache: PathBuf,
    /// How events are written.
    pub output: Output,
    /// The builder's stamp (`hello` says it).
    pub builder: String,
    /// Asks the player in the builder's own window ([`BuilderCx::confirm`]).
    pub confirm: Arc<dyn Fn(&Confirmation) -> bool + Send + Sync>,
    /// Set when the game asks to be built again (`restart_to_apply`): the caller reads it once the game has exited,
    /// whether or not the serving thread has seen the stream's end (something the game started may hold its end).
    pub restart: Arc<AtomicBool>,
}

/// The hook's answer to one request: its result, or why not (`events` says progress as it goes).
pub type Hook = dyn Fn(&Request, &BuilderCx, &dyn Fn(Event)) -> Result<Value, OpError> + Send + Sync;

/// What a served game asked of the builder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Served {
    /// The game asked to be built again (`restart_to_apply`). The builder builds and starts it again when it then exits
    /// with status 0 (no exit code of its own).
    pub restart_requested: bool,
    /// Requests answered.
    pub requests: u32,
}

struct Shared {
    out: Mutex<Box<dyn Write + Send>>,
    closed: AtomicBool,
    pending: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    restart: Arc<AtomicBool>,
    seen: Mutex<Option<(SystemTime, u64)>>,
    cfg: Config,
}

impl Shared {
    fn send(&self, v: &Value) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut line = v.to_string();
        line.push('\n');
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        if out.write_all(line.as_bytes()).and_then(|()| out.flush()).is_err() {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    fn send_event(&self, e: &Event) {
        if let Ok(v) = serde_json::to_value(e) {
            self.send(&v);
        }
    }

    fn stamp(&self) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(self.cfg.profile.join(launcher_profile::FILE)).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    /// Says `profile_changed` when profile.json is not as it was when last looked at.
    fn watch(&self) {
        let now = self.stamp();
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if *seen != now {
            *seen = now;
            drop(seen);
            self.send_event(&Event::ProfileChanged);
        }
    }

    fn rebase(&self) {
        *self.seen.lock().unwrap_or_else(|p| p.into_inner()) = self.stamp();
    }
}

/// How a line ended.
enum Line {
    Got(Vec<u8>),
    End,
    TooLong,
}

fn read_line(r: &mut impl BufRead) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(LINE_MAX as u64 + 2).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(Line::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    // (more than the limit before its newline, or no newline within it: too long)
    if buf.len() > LINE_MAX {
        return Ok(Line::TooLong);
    }
    Ok(Line::Got(buf))
}

/// Whether `op` is a name an op can have.
fn is_op_name(op: &str) -> bool {
    !op.is_empty() && op.len() <= 32 && op.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Serves the game on `reader` and `writer` until the stream ends, `stop` is set (the game ended) or a line is too long,
/// the hook being [`crate::modhook::serve`].
pub fn serve(cfg: Config, reader: impl Read, writer: impl Write + Send + 'static, stop: &AtomicBool) -> Served {
    serve_with(cfg, reader, writer, stop, Arc::new(crate::modhook::serve), crate::modhook::ops())
}

/// [`serve`] with another hook and the names it serves.
pub fn serve_with(
    cfg: Config,
    reader: impl Read,
    writer: impl Write + Send + 'static,
    stop: &AtomicBool,
    hook: Arc<Hook>,
    hook_ops: &[&str],
) -> Served {
    let shared = Arc::new(Shared {
        out: Mutex::new(Box::new(writer)),
        closed: AtomicBool::new(false),
        pending: Mutex::new(HashMap::new()),
        restart: cfg.restart.clone(),
        seen: Mutex::new(None),
        cfg,
    });
    shared.rebase();
    let hook_ops: Vec<String> = hook_ops.iter().map(|s| s.to_string()).collect();
    let mut reader = std::io::BufReader::new(reader);
    let (mut said_hello, mut requests) = (false, 0u32);
    while !stop.load(Ordering::SeqCst) && !shared.closed.load(Ordering::SeqCst) {
        let bytes = match read_line(&mut reader) {
            Ok(Line::Got(b)) => b,
            Ok(Line::End) | Err(_) => break,
            Ok(Line::TooLong) => {
                shared.send(&Reply::err("refused", format!("a line is at most {LINE_MAX} bytes")).line(None));
                break;
            }
        };
        if stop.load(Ordering::SeqCst) {
            break;
        }
        requests += 1;
        shared.watch();
        let Ok(Value::Object(req)) = serde_json::from_slice::<Value>(&bytes) else {
            shared.send(&Reply::err("bad_args", "a request is a JSON object on one line").line(None));
            continue;
        };
        let id = req.get("id").and_then(Value::as_u64);
        let op = req.get("op").and_then(Value::as_str).unwrap_or("");
        if !is_op_name(op) {
            let why = if op.is_empty() { "a request has an op" } else { "no such op" };
            let code = if op.is_empty() { "bad_args" } else { "unknown_op" };
            shared.send(&Reply::err(code, why).line(id));
            continue;
        }
        let args = match req.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(a @ Value::Object(_)) => a.clone(),
            Some(_) => {
                shared.send(&Reply::err("bad_args", "args is an object").line(id));
                continue;
            }
        };
        if op == "hello" {
            let reply = if req.get("v").and_then(Value::as_u64) == Some(VERSION) {
                said_hello = true;
                let mut ops: Vec<&str> = BUILT_IN.to_vec();
                ops.extend(hook_ops.iter().map(String::as_str));
                Reply::Ok(json!({"v": VERSION, "builder": shared.cfg.builder, "ops": ops}))
            } else {
                Reply::err("refused", format!("this builder speaks version {VERSION}"))
            };
            shared.send(&reply.line(id));
            continue;
        }
        let Some(id) = id else {
            shared.send(&Reply::err("bad_args", "a request has an id").line(None));
            continue;
        };
        if !said_hello {
            shared.send(&Reply::err("refused", "the first request is hello").line(Some(id)));
            continue;
        }
        match op {
            "profile" => shared.send(&profile_reply(&shared.cfg).line(Some(id))),
            "set_mods" => {
                let r = set_mods_reply(&shared, &args);
                shared.send(&r.line(Some(id)));
            }
            "restart_to_apply" => {
                shared.restart.store(true, Ordering::SeqCst);
                shared.send(&Reply::Ok(json!({})).line(Some(id)));
            }
            "cancel" => {
                let flag = args
                    .get("id")
                    .and_then(Value::as_u64)
                    .and_then(|t| shared.pending.lock().unwrap_or_else(|p| p.into_inner()).get(&t).cloned());
                if let Some(f) = &flag {
                    f.store(true, Ordering::SeqCst);
                }
                shared.send(&Reply::Ok(json!({"cancelled": flag.is_some()})).line(Some(id)));
            }
            _ => {
                let cancelled = Arc::new(AtomicBool::new(false));
                {
                    let mut pending = shared.pending.lock().unwrap_or_else(|p| p.into_inner());
                    if pending.len() >= PENDING_MAX || pending.contains_key(&id) {
                        let (code, text) = if pending.contains_key(&id) {
                            ("bad_args", "that id is waiting for its answer already")
                        } else {
                            ("busy", "too many requests are waiting")
                        };
                        drop(pending);
                        shared.send(&Reply::err(code, text).line(Some(id)));
                        continue;
                    }
                    pending.insert(id, cancelled.clone());
                }
                let request = Request { id, op: op.to_string(), args, cancelled };
                let (s, hook) = (shared.clone(), hook.clone());
                std::thread::spawn(move || {
                    // (a game can't start a game: play is refused here)
                    let nop = |_| ExitCode::FAILURE;
                    let ask = |c: &Confirmation| (s.cfg.confirm)(c);
                    let cx = BuilderCx::resolve(
                        &s.cfg.version,
                        Some(&s.cfg.profile),
                        s.cfg.cache.clone(),
                        s.cfg.output,
                        &nop,
                        &ask,
                    );
                    let events = |e: Event| s.send_event(&e);
                    let reply = Reply::of(hook(&request, &cx, &events));
                    s.pending.lock().unwrap_or_else(|p| p.into_inner()).remove(&request.id);
                    s.send(&reply.line(Some(request.id)));
                });
            }
        }
    }
    shared.closed.store(true, Ordering::SeqCst);
    Served { restart_requested: shared.restart.load(Ordering::SeqCst), requests }
}

fn profile_reply(cfg: &Config) -> Reply {
    let layout = launcher_profile::Layout::new(&cfg.profile);
    let p = match layout.read() {
        Ok(p) => p,
        Err(e) => return Reply::err("refused", e.to_string()),
    };
    let mods: Value = p.raw("mods").and_then(|m| serde_json::from_str(m.get()).ok()).unwrap_or(Value::Null);
    Reply::Ok(json!({
        "dir": layout.dir(),
        "name": p.name(),
        "color": p.color(),
        "version": p.version(),
        "game": p.game(),
        "start_slot": p.start_slot().and_then(Result::ok),
        "upscalers": p.upscaler_names(),
        "mods": mods,
    }))
}

fn set_mods_reply(shared: &Shared, args: &Value) -> Reply {
    let Some(mods) = args.get("mods") else {
        return Reply::err("bad_args", "set_mods takes args.mods");
    };
    let layout = launcher_profile::Layout::new(&shared.cfg.profile);
    let _lock = match launcher_profile::EditGuard::take(layout.dir(), EDIT_WAIT) {
        Ok(l) => l,
        Err(launcher_profile::LockError::Busy) => {
            return Reply::err("busy", "another change to the profile is going on");
        }
        Err(e) => return Reply::err("refused", e.to_string()),
    };
    let mut p = match layout.read() {
        Ok(p) => p,
        Err(e) => return Reply::err("refused", e.to_string()),
    };
    if let Err(e) = launcher_profile::set_mods(&mut p, mods) {
        return Reply::err(e.code, e.text);
    }
    if let Err(e) = p.write(&layout.file()) {
        return Reply::err("refused", e.to_string());
    }
    // (the game's own change: not told back to it)
    shared.rebase();
    Reply::Ok(json!({}))
}

#[cfg(test)]
mod tests;
