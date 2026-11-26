use super::*;
use std::io::{BufRead, BufReader, PipeReader, PipeWriter};
use std::path::Path;

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/test-scratch")
        .join(format!("channel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A profile folder with the mods the tests change.
fn profile(name: &str) -> (PathBuf, Config) {
    let d = scratch(name);
    let prof = d.join("profile");
    std::fs::create_dir_all(&prof).unwrap();
    std::fs::write(
        prof.join("profile.json"),
        r#"{"v":1,"name":"Ann","mods":[{"id":"a","on":true,"granted":["network"]},{"id":"b","on":false}]}"#,
    )
    .unwrap();
    let cfg = Config {
        profile: prof,
        version: d.join("v"),
        cache: d.join("cache"),
        output: Output::Json,
        confirm: Arc::new(|_| false),
        builder: "test-builder".into(),
        restart: Arc::default(),
    };
    (d, cfg)
}

/// The game's side of a served channel.
struct Game {
    to: Option<PipeWriter>,
    from: BufReader<PipeReader>,
    server: Option<std::thread::JoinHandle<Served>>,
}

fn hook_of(
    f: impl Fn(&Request, &BuilderCx, &dyn Fn(Event)) -> Result<Value, OpError> + Send + Sync + 'static,
) -> Arc<Hook> {
    Arc::new(f)
}

impl Game {
    fn start(cfg: Config, hook: Arc<Hook>, ops: &'static [&'static str]) -> Game {
        let (server_in, to) = std::io::pipe().unwrap();
        let (from, server_out) = std::io::pipe().unwrap();
        let server =
            std::thread::spawn(move || serve_with(cfg, server_in, server_out, &AtomicBool::new(false), hook, ops));
        Game { to: Some(to), from: BufReader::new(from), server: Some(server) }
    }

    fn plain(cfg: Config) -> Game {
        Game::start(cfg, hook_of(|_, _, _| Err(OpError::new("unknown_op", "no such op"))), &[])
    }

    fn say(&mut self, line: &str) {
        let to = self.to.as_mut().unwrap();
        to.write_all(line.as_bytes()).unwrap();
        to.write_all(b"\n").unwrap();
    }

    fn hear(&mut self) -> Value {
        let mut line = String::new();
        self.from.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "the builder closed the channel");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
    }

    fn ask(&mut self, line: &str) -> Value {
        self.say(line);
        self.hear()
    }

    fn hello(&mut self) -> Value {
        self.ask(r#"{"id":1,"op":"hello","v":1}"#)
    }

    /// The game ends: its end closes; what the builder served.
    fn end(&mut self) -> Served {
        self.to = None;
        self.server.take().unwrap().join().unwrap()
    }
}

#[test]
fn hello_answers_the_version_the_builder_and_the_ops() {
    let (d, cfg) = profile("hello");
    let mut g = Game::start(cfg, hook_of(|_, _, _| Err(OpError::new("unknown_op", "no such op"))), &["install"]);
    let r = g.hello();
    assert_eq!(r["id"], 1);
    assert_eq!(r["ok"], true);
    assert_eq!(r["result"]["v"], 1);
    assert_eq!(r["result"]["builder"], "test-builder");
    assert_eq!(r["result"]["ops"], json!(["hello", "profile", "set_mods", "restart_to_apply", "cancel", "install"]));
    // (the id is optional in hello: the reply says null)
    assert_eq!(g.ask(r#"{"op":"hello","v":1}"#)["id"], Value::Null);
    // (a version it does not speak)
    let r = g.ask(r#"{"id":2,"op":"hello","v":2}"#);
    assert_eq!((r["ok"].as_bool(), r["code"].as_str()), (Some(false), Some("refused")));
    assert_eq!(g.end().requests, 3);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_first_request_is_hello_and_every_other_is_typed_and_checked() {
    let (d, cfg) = profile("forged");
    let mut g = Game::plain(cfg);
    assert_eq!(g.ask(r#"{"id":1,"op":"profile"}"#)["code"], "refused");
    g.hello();
    for (line, code) in [
        ("not json", "bad_args"),
        ("[1,2]", "bad_args"),
        ("\"x\"", "bad_args"),
        (r#"{"id":2}"#, "bad_args"),
        (r#"{"id":2,"op":7}"#, "bad_args"),
        (r#"{"op":"profile"}"#, "bad_args"),
        (r#"{"id":2,"op":"profile","args":[1]}"#, "bad_args"),
        (r#"{"id":2,"op":"fetch_url","args":{"url":"https://x"}}"#, "unknown_op"),
        (r#"{"id":2,"op":"../../etc"}"#, "unknown_op"),
        (r#"{"id":2,"op":"UPPER"}"#, "unknown_op"),
    ] {
        let r = g.ask(line);
        assert_eq!((r["ok"].as_bool(), r["code"].as_str()), (Some(false), Some(code)), "{line}: {r}");
    }
    // (an op the hook does not know is the hook's to refuse: unknown_op)
    assert_eq!(g.ask(r#"{"id":3,"op":"install","args":{}}"#)["code"], "unknown_op");
    // (still serving)
    assert_eq!(g.ask(r#"{"id":4,"op":"profile"}"#)["ok"], true);
    g.end();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_stub_hook_says_the_mod_tooling_is_missing() {
    let (d, cfg) = profile("stub");
    let (server_in, mut to) = std::io::pipe().unwrap();
    let (from, server_out) = std::io::pipe().unwrap();
    let t = std::thread::spawn(move || serve(cfg, server_in, server_out, &AtomicBool::new(false)));
    to.write_all(b"{\"id\":1,\"op\":\"hello\",\"v\":1}\n{\"id\":2,\"op\":\"install\",\"args\":{}}\n").unwrap();
    let mut from = BufReader::new(from);
    let mut line = String::new();
    from.read_line(&mut line).unwrap();
    line.clear();
    from.read_line(&mut line).unwrap();
    let r: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        (r["id"].as_u64(), r["ok"].as_bool(), r["code"].as_str()),
        (Some(2), Some(false), Some("mod_tooling_missing"))
    );
    drop(to);
    t.join().unwrap();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_line_over_the_limit_closes_the_channel() {
    let (d, cfg) = profile("long");
    let mut g = Game::plain(cfg);
    g.hello();
    let long = format!("{{\"id\":2,\"op\":\"profile\",\"args\":{{\"x\":\"{}\"}}}}", "a".repeat(LINE_MAX));
    // (the builder stops reading at the limit and may close its end before the rest of the line is written: a broken
    // pipe here is the builder hanging up, as it should, not a failure)
    let to = g.to.as_mut().unwrap();
    match to.write_all(long.as_bytes()).and_then(|()| to.write_all(b"\n")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("writing the long line: {e}"),
    }
    let r = g.hear();
    assert_eq!(r["code"], "refused");
    // (then nothing more is served: the stream ends)
    let mut rest = String::new();
    assert_eq!(g.from.read_line(&mut rest).unwrap(), 0);
    g.server.take().unwrap().join().unwrap();
    // (a line of exactly the limit is served)
    let (_, cfg) = profile("long-ok");
    let mut g = Game::plain(cfg);
    g.hello();
    let pad = LINE_MAX - r#"{"id":2,"op":"profile","args":{"x":""}}"#.len();
    let line = format!("{{\"id\":2,\"op\":\"profile\",\"args\":{{\"x\":\"{}\"}}}}", "a".repeat(pad));
    assert_eq!(line.len(), LINE_MAX);
    assert_eq!(g.ask(&line)["ok"], true);
    g.end();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn profile_says_the_folder_and_the_known_keys() {
    let (d, cfg) = profile("profile");
    let dir = cfg.profile.clone();
    let mut g = Game::plain(cfg);
    g.hello();
    let r = g.ask(r#"{"id":2,"op":"profile"}"#);
    assert_eq!(r["result"]["dir"], dir.to_str().unwrap());
    assert_eq!(r["result"]["name"], "Ann");
    assert_eq!(r["result"]["mods"][0]["id"], "a");
    g.end();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn set_mods_changes_the_order_and_on_and_never_a_mods_permissions() {
    let (d, cfg) = profile("set-mods");
    let file = cfg.profile.join("profile.json");
    let mut g = Game::plain(cfg);
    g.hello();
    // (forged: a granted of its own, and a mod that is not there)
    let r = g.ask(r#"{"id":2,"op":"set_mods","args":{"mods":[{"id":"b","on":true,"granted":["everything"]},{"id":"a","on":false}]}}"#);
    assert_eq!(r["ok"], true, "{r}");
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        after["mods"],
        json!([{"id":"b","on":true},{"id":"a","on":false,"granted":["network"]}]),
        "order and on; a's granted kept, b's none added"
    );
    let before = std::fs::read_to_string(&file).unwrap();
    let r = g.ask(r#"{"id":3,"op":"set_mods","args":{"mods":[{"id":"nope","on":true}]}}"#);
    assert_eq!(r["code"], "not_found");
    assert_eq!(g.ask(r#"{"id":4,"op":"set_mods","args":{}}"#)["code"], "bad_args");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before, "refused: unchanged");
    // (an edit going on: busy, nothing written)
    let held = launcher_profile::EditGuard::take(file.parent().unwrap(), Duration::ZERO).unwrap();
    assert_eq!(g.ask(r#"{"id":5,"op":"set_mods","args":{"mods":[{"id":"a","on":true}]}}"#)["code"], "busy");
    drop(held);
    assert_eq!(g.ask(r#"{"id":6,"op":"set_mods","args":{"mods":[{"id":"a","on":true}]}}"#)["ok"], true);
    assert!(!file.parent().unwrap().join(launcher_profile::EDIT_LOCK).exists(), "the lock is let go");
    // (the game's own change is not told back to it, an other's is)
    std::fs::write(&file, r#"{"v":1,"name":"Ann B","mods":[]}"#).unwrap();
    let r = g.ask(r#"{"id":7,"op":"profile"}"#);
    assert_eq!(r["event"], "profile_changed", "said before the reply: {r}");
    assert_eq!(g.hear()["result"]["name"], "Ann B");
    g.end();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn restart_to_apply_is_noted() {
    let (d, cfg) = profile("restart");
    let mut g = Game::plain(cfg);
    g.hello();
    assert_eq!(g.ask(r#"{"id":2,"op":"restart_to_apply"}"#)["ok"], true);
    assert_eq!(g.end(), Served { restart_requested: true, requests: 2 });
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn too_many_pending_requests_are_busy_and_cancel_stops_one_by_its_id() {
    let (d, cfg) = profile("busy");
    // (an op that waits until it is cancelled, said as it goes)
    let hook = hook_of(|req, _, events| {
        events(Event::Progress { id: req.id, fraction: 0.5, text: "waiting".into() });
        while !req.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(OpError::new("cancelled", "stopped"))
    });
    let mut g = Game::start(cfg, hook, &["slow"]);
    g.hello();
    for id in 10..10 + PENDING_MAX as u64 {
        g.say(&format!(r#"{{"id":{id},"op":"slow"}}"#));
    }
    // (their progress events: one each, in any order with the rest)
    for _ in 0..PENDING_MAX {
        let e = g.hear();
        assert_eq!(e["event"], "progress");
        assert!(e["id"].as_u64().is_some_and(|i| (10..10 + PENDING_MAX as u64).contains(&i)));
    }
    let r = g.ask(r#"{"id":99,"op":"slow"}"#);
    assert_eq!((r["id"].as_u64(), r["code"].as_str()), (Some(99), Some("busy")));
    // (the same id again while it waits)
    assert_eq!(g.ask(r#"{"id":10,"op":"slow"}"#)["code"], "bad_args");
    // (cancel one: its reply and cancel's own)
    g.say(r#"{"id":50,"op":"cancel","args":{"id":12}}"#);
    let (a, b) = (g.hear(), g.hear());
    let (cancel, slow) = if a["id"] == 50 { (a, b) } else { (b, a) };
    assert_eq!(cancel["result"]["cancelled"], true);
    assert_eq!((slow["id"].as_u64(), slow["code"].as_str()), (Some(12), Some("cancelled")));
    // (an id that waits for nothing)
    assert_eq!(g.ask(r#"{"id":51,"op":"cancel","args":{"id":777}}"#)["result"]["cancelled"], false);
    // (room again: one more is taken)
    g.say(r#"{"id":98,"op":"slow"}"#);
    assert_eq!(g.hear()["event"], "progress");
    // (the rest end: cancelled each)
    for id in (10..10 + PENDING_MAX as u64).filter(|i| *i != 12).chain([98]) {
        g.say(&format!(r#"{{"id":{},"op":"cancel","args":{{"id":{id}}}}}"#, 1000 + id));
    }
    let mut cancelled = 0;
    while cancelled < PENDING_MAX {
        let r = g.hear();
        if r["code"] == "cancelled" {
            cancelled += 1;
        }
    }
    g.end();
    std::fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn a_game_process_says_hello_over_the_ends_it_inherits() {
    use std::process::Command;
    let (d, cfg) = profile("process");
    let (server, child) = build_core::channel::open().unwrap();
    let out = d.join("heard.txt");
    // (the game: hello out of its write end, the answer into a file, then a request that is not allowed)
    // (bash: dash, CI's sh, takes no fd number above 9 in >&$w, and the channel's ends may be)
    let mut game = Command::new("bash");
    game.args([
        "-c",
        &format!(
            "r=${{LONGSHOT_BUILDER_CHANNEL%,*}}; w=${{LONGSHOT_BUILDER_CHANNEL#*,}}; \
             echo '{{\"id\":1,\"op\":\"hello\",\"v\":1}}' >&$w; read a <&$r; echo \"$a\" > '{o}'; \
             echo '{{\"id\":2,\"op\":\"nope\"}}' >&$w; read b <&$r; echo \"$b\" >> '{o}'",
            o = out.display()
        ),
    ]);
    child.prepare(&mut game);
    let mut running = game.spawn().unwrap();
    drop(child);
    let stop = AtomicBool::new(false);
    let served = serve(cfg, server.reader, server.writer, &stop);
    assert!(running.wait().unwrap().success());
    // (the stream ended with the game: its write end closed, the builder's copy of it too)
    assert_eq!(served.requests, 2);
    let heard = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<Value> = heard.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["result"]["v"], 1);
    assert_eq!(lines[1]["code"], "mod_tooling_missing");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn nothing_is_said_after_the_game_ends() {
    let (d, cfg) = profile("stop");
    let (server_in, mut to) = std::io::pipe().unwrap();
    let (from, server_out) = std::io::pipe().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let t = std::thread::spawn(move || serve(cfg, server_in, server_out, &s2));
    to.write_all(b"{\"id\":1,\"op\":\"hello\",\"v\":1}\n").unwrap();
    let mut from = BufReader::new(from);
    let mut line = String::new();
    from.read_line(&mut line).unwrap();
    // (the game ended: stop is set, and the next line read is not served)
    stop.store(true, Ordering::SeqCst);
    to.write_all(b"{\"id\":2,\"op\":\"profile\"}\n").unwrap();
    assert_eq!(t.join().unwrap().requests, 1);
    drop(to);
    line.clear();
    assert_eq!(from.read_line(&mut line).unwrap(), 0, "the builder's end is closed");
    std::fs::remove_dir_all(d).unwrap();
}
