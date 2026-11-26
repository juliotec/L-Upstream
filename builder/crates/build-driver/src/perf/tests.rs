//! The launcher's side of the performance statistics, against a stand-in relay on this PC (a TCP listener that
//! answers each POST with the next status it is given): the tick, the order, the version and release filled in, what
//! each answer does, the busy wait, the caps, the copies, the send lock. curl does the POSTs, as for a player; without
//! curl on the PATH the tests that post say so and pass.

use super::*;
use std::collections::HashMap;
use std::io::BufRead;
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// What the stand-in relay saw.
#[derive(Default)]
struct Seen {
    bodies: Vec<serde_json::Value>,
    methods: Vec<String>,
}

/// A relay on 127.0.0.1: each request answered with the next of `codes` (then 201) and `headers`.
struct Relay {
    url: String,
    seen: Arc<Mutex<Seen>>,
}

impl Relay {
    fn new(codes: &[u16], headers: &[(&str, &str)]) -> Relay {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/perf", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (codes, headers): (Vec<u16>, Vec<(String, String)>) =
            (codes.to_vec(), headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        let s = seen.clone();
        std::thread::spawn(move || {
            let mut codes = codes.into_iter();
            for stream in listener.incoming().flatten() {
                let mut r = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    continue;
                }
                let method = line.split_whitespace().next().unwrap_or("").to_string();
                let mut length = 0;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':')
                        && k.eq_ignore_ascii_case("content-length")
                    {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = r.read_exact(&mut body);
                {
                    let mut s = s.lock().unwrap();
                    s.methods.push(method);
                    s.bodies.push(serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null));
                }
                let code = codes.next().unwrap_or(201);
                let mut out = format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n");
                for (k, v) in &headers {
                    out += &format!("{k}: {v}\r\n");
                }
                out += "\r\n";
                let mut stream = stream;
                let _ = stream.write_all(out.as_bytes());
            }
        });
        Relay { url, seen }
    }

    fn ns(&self) -> Vec<i64> {
        self.seen.lock().unwrap().bodies.iter().map(|b| b["fps_cap"].as_i64().unwrap_or(-1)).collect()
    }
}

fn have_curl() -> bool {
    let ok = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" })
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skip: no curl on the PATH");
    }
    ok
}

fn scratch(name: &str) -> PathBuf {
    let d = crate::scratch(&format!("perf-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(PENDING)).unwrap();
    d
}

/// A game folder with `summaries` waiting (`n` 0, 1, ... oldest first), the tick on, a release's version and source.
fn folder(name: &str, summaries: usize) -> PathBuf {
    let f = scratch(name);
    set_tick(&f, true, true).unwrap();
    std::fs::write(f.join(VERSION_FILE), "version=v3.1.0\nsource=abcd1234\nperf_reports=https://relay.example/perf\n")
        .unwrap();
    std::fs::create_dir_all(f.join("logs")).unwrap();
    std::fs::write(f.join("logs/longshot_log.txt"), "[0.0] longshot v3.1.0 (source abcd1234) starting\n").unwrap();
    for n in 0..summaries {
        std::fs::write(
            f.join(PENDING).join(format!("{}-{n}.json", 1_700_000_000 + n)),
            format!("{{\"v\":2,\"fps_cap\":{n}}}"),
        )
        .unwrap();
    }
    f
}

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |k| m.get(k).cloned()
}

fn send_to(f: &Path, relay: &Relay, more: &[(&str, &str)]) -> Status {
    let mut pairs = vec![("LONGSHOT_PERF_URL", relay.url.as_str())];
    pairs.extend_from_slice(more);
    send(f, &env_of(&pairs))
}

#[test]
fn the_tick_and_what_off_deletes() {
    let f = scratch("tick");
    assert!(!ticked(&f), "off by default");
    set_tick(&f, true, true).unwrap();
    assert_eq!(std::fs::read_to_string(f.join(CHOICE)).unwrap(), "send=1\n");
    set_tick(&f, true, false).unwrap();
    assert_eq!(std::fs::read_to_string(f.join(CHOICE)).unwrap(), "send=1\nkeep_sent=0\n");
    assert!(!keeping_sent(&f, &env_of(&[])));
    assert!(keeping_sent(&f, &env_of(&[("LONGSHOT_PERF_KEEP_SENT", "on")])), "the switch wins over keep_sent=0");
    std::fs::write(f.join(PENDING).join("1-0.json"), "{}").unwrap();
    std::fs::create_dir_all(f.join(SENT)).unwrap();
    std::fs::write(f.join(SENT).join("1-0.json"), "{}").unwrap();
    set_tick(&f, false, true).unwrap();
    assert!(
        !f.join(CHOICE).exists() && waiting(&f).is_empty() && listed(&f, SENT).is_empty(),
        "off: the player said no"
    );
    let _ = std::fs::remove_dir_all(&f);
}

#[test]
fn the_folder_facts() {
    let f = folder("facts", 0);
    assert_eq!(build_facts(&f), ("v3.1.0".into(), true, "https://relay.example/perf".into()));
    std::fs::write(f.join("logs/longshot_log.txt"), "longshot v3.1.0 (source ffff0000)\n").unwrap();
    assert!(!build_facts(&f).1, "built from other sources: not a release");
    std::fs::write(f.join(VERSION_FILE), "version=v3.1.0-4-gabc\nsource=ffff0000\nperf_reports=http://x/\n").unwrap();
    let (v, release, relay) = build_facts(&f);
    assert_eq!(
        (v.as_str(), release, relay.as_str()),
        ("v3.1.0-4-gabc", false, ""),
        "not a release's name; no https: nowhere"
    );
    for (v, ok) in [
        ("v3.1.0", true),
        ("3.1.0-rc.2", true),
        ("nightly-12-0123abcd", true),
        ("nightly-12-0123ABCD", false),
        ("v3.1", false),
    ] {
        assert_eq!(release_version(v), ok, "{v}");
    }
    let env = env_of(&[("LONGSHOT_PERF_URL", "http://10.0.0.2/perf")]);
    assert_eq!(endpoint(&env, "https://a/"), "https://a/", "plain http to another PC: not taken");
    assert_eq!(
        endpoint(&env_of(&[("LONGSHOT_PERF_URL", "http://127.0.0.1:8080/perf")]), ""),
        "http://127.0.0.1:8080/perf"
    );
    let _ = std::fs::remove_dir_all(&f);
}

#[test]
fn the_answers() {
    assert_eq!(answer(201, None), Answer::Sent);
    for c in [400, 413, 415, 422] {
        assert_eq!(answer(c, None), Answer::Refused);
    }
    for c in [301, 302, 307, 308, 404, 408, 425] {
        assert_eq!(answer(c, None), Answer::Later, "{c}");
    }
    assert_eq!(answer(429, Some("120")), Answer::Busy(Some(120)));
    assert_eq!(answer(503, None), Answer::Busy(None));
    assert!(wait_of("Wed, 21 Oct 2099 07:28:00 GMT").unwrap() > 86_400);
    assert_eq!(wait_of("Wed, 21 Oct 2015 07:28:00 GMT"), Some(0), "a date gone by: now");
    assert_eq!(wait_of("soon"), None);
}

#[test]
fn sent_oldest_first_with_the_version_and_kept() {
    if !have_curl() {
        return;
    }
    let f = folder("send", 3);
    std::fs::write(f.join(PENDING).join("1699999999-9.json"), "{\"v\":2,\"fps_cap\":9,\"version\":\"v3.0.0\"}")
        .unwrap();
    let relay = Relay::new(&[], &[]);
    let s = send_to(&f, &relay, &[]);
    assert_eq!((s.sent, s.refused, s.waiting), (4, 0, 0));
    assert_eq!(relay.ns(), vec![9, 0, 1, 2], "oldest first");
    let seen = relay.seen.lock().unwrap();
    assert!(seen.methods.iter().all(|m| m == "POST"));
    assert_eq!(
        (seen.bodies[0]["version"].as_str(), seen.bodies[0]["release"].as_bool()),
        (Some("v3.0.0"), Some(false)),
        "the game's own version: not this folder's release"
    );
    assert!(seen.bodies[1..].iter().all(|b| b["version"] == "v3.1.0" && b["release"] == true));
    assert_eq!(listed(&f, SENT).len(), 4, "each kept as sent");
    let _ = std::fs::remove_dir_all(&f);
}

#[test]
fn busy_refused_and_later() {
    if !have_curl() {
        return;
    }
    // (a 400 drops that one; a 429 with Retry-After stops, noted, and the rest stays)
    let f = folder("busy", 4);
    let relay = Relay::new(&[201, 400, 429], &[("Retry-After", "120")]);
    let s = send_to(&f, &relay, &[]);
    assert_eq!((s.sent, s.refused, s.waiting), (1, 1, 2));
    let (at, wait) = busy_until(&f);
    assert!(wait == 120 && at - now() > 100.0);
    assert!(s.busy_until.is_some());
    let quiet = Relay::new(&[], &[]);
    let s = send_to(&f, &quiet, &[]);
    assert!(quiet.ns().is_empty() && s.waiting == 2, "while the relay asked to be left alone: nothing asked");
    let s = send_to(&f, &quiet, &[("LONGSHOT_PERF_BACKOFF", "0")]);
    assert!(s.sent == 2 && !pending(&f, NEXT).exists(), "the wait off: sent, and the note gone once all went");
    // (busy again without a Retry-After: an hour, doubled each time, up to a day)
    note_busy(&f, None);
    let first = busy_until(&f).1;
    note_busy(&f, None);
    assert_eq!((first, busy_until(&f).1), (3600, 7200));
    // (not reached: everything stays)
    let g = folder("later", 2);
    let gone = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/perf", gone.local_addr().unwrap().port());
    drop(gone);
    let s = send(&g, &env_of(&[("LONGSHOT_PERF_URL", &url)]));
    assert_eq!((s.sent, s.waiting), (0, 2));
    // (a redirect is not followed: its 3xx is the answer, and the summary stays)
    let h = folder("redirect", 1);
    let relay = Relay::new(&[302], &[("Location", "/elsewhere")]);
    let s = send_to(&h, &relay, &[]);
    assert_eq!((s.sent, s.waiting, relay.seen.lock().unwrap().methods.clone()), (0, 1, vec!["POST".to_string()]));
    for d in [f, g, h] {
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn the_caps() {
    if !have_curl() {
        return;
    }
    let f = folder("caps", 20);
    let relay = Relay::new(&[], &[]);
    let s = send_to(&f, &relay, &[]);
    assert_eq!((s.sent, s.waiting), (MAX_SENDS, 20 - MAX_SENDS), "at most 5 a start");
    let s = send_to(&f, &relay, &[("LONGSHOT_PERF_MAX_SENDS", "3")]);
    assert_eq!(s.sent, 3);
    let g = folder("many", 53);
    let relay = Relay::new(&[], &[]);
    let s = send_to(&g, &relay, &[("LONGSHOT_PERF_MAX_SENDS", "48")]);
    assert_eq!((s.sent, relay.ns()[0]), (MAX_PENDING, 5), "at most 48 wait: the oldest go unsent");
    // (too big, or not a summary: dropped without asking)
    let h = folder("big", 0);
    std::fs::write(pending(&h, "1-0.json"), format!("{{\"v\":2,\"os\":\"{}\"}}", "x".repeat(MAX_BODY))).unwrap();
    std::fs::write(pending(&h, "1-1.json"), "[1]").unwrap();
    std::fs::write(pending(&h, "1-2.json"), b"{\"v\":2,\"os\":\"\xff\",\"fps_cap\":2}").unwrap();
    let relay = Relay::new(&[], &[]);
    let s = send_to(&h, &relay, &[]);
    assert_eq!((s.sent, s.refused, relay.ns()), (1, 2, vec![2]), "a broken character is one field's text");
    // (copies off: sent, none kept)
    let k = folder("nocopy", 2);
    set_tick(&k, true, false).unwrap();
    let s = send_to(&k, &Relay::new(&[], &[]), &[]);
    assert!(s.sent == 2 && listed(&k, SENT).is_empty());
    // (the tick off: nothing sent)
    let t = folder("off", 2);
    set_tick(&t, false, true).unwrap();
    let relay = Relay::new(&[], &[]);
    assert_eq!(send_to(&t, &relay, &[]).sent, 0);
    assert!(relay.ns().is_empty());
    for d in [f, g, h, k, t] {
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn one_sender_at_a_time() {
    let f = folder("lock", 2);
    let lock = pending(&f, LOCK);
    let posted = Mutex::new(0);
    let poster = |_: &[u8], _: &str, _: u64| {
        *posted.lock().unwrap() += 1;
        Answer::Sent
    };
    let env = env_of(&[("LONGSHOT_PERF_URL", "https://relay.example/perf")]);
    // (a running holder with a lease: refused, nothing asked)
    std::fs::write(&lock, format!("{} {}\n", std::process::id(), now() as u64 + LEASE)).unwrap();
    let s = send_with(&f, &env, &poster, Duration::from_millis(20));
    assert_eq!((s.sent, *posted.lock().unwrap(), s.waiting), (0, 0, 2));
    // (a holder certainly gone, mid-lease: taken over, and the lock released after)
    #[cfg(target_os = "linux")]
    {
        let dead = Command::new("true").spawn().and_then(|mut c| c.wait().map(|_| c.id())).unwrap();
        std::fs::write(&lock, format!("{dead} {}\n", now() as u64 + LEASE)).unwrap();
        let s = send_with(&f, &env, &poster, Duration::from_millis(20));
        assert_eq!(s.sent, 2);
        assert!(!lock.exists(), "the holder's own release clears it");
    }
    // (a lock left as a directory: cleared)
    std::fs::create_dir_all(&lock).unwrap();
    assert!(take_send_lock(&f));
    assert_eq!(lock_read(&lock).map(|l| l.0), Some(std::process::id()));
    release_send_lock(&f);
    // (an empty body, fresh: a sender mid-create holds)
    std::fs::write(&lock, "").unwrap();
    assert!(!take_send_lock(&f));
    // (the heartbeat renews our lease while we hold it)
    std::fs::write(&lock, format!("{} 1\n", std::process::id())).unwrap();
    let (tx, rx) = mpsc::channel();
    let g = f.clone();
    let beat = std::thread::spawn(move || heartbeat(g, rx, Duration::from_millis(10)));
    std::thread::sleep(Duration::from_millis(60));
    let _ = tx.send(());
    beat.join().unwrap();
    assert!(lock_read(&lock).unwrap().1 > now() as u64, "renewed");
    let _ = std::fs::remove_dir_all(&f);
}

/// A release's tree (its VERSION) naming `relay` in its addresses.ini.
fn release_tree(name: &str, relay: &str) -> PathBuf {
    let t = scratch(&format!("{name}-tree"));
    for d in ["host", "game", "gpu/vk", "third_party"] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    std::fs::write(t.join("VERSION"), "v3.1.0\n").unwrap();
    std::fs::create_dir_all(t.join("assets")).unwrap();
    std::fs::write(t.join("assets/addresses.ini"), format!("[addresses]\nperf_reports = {relay}\n")).unwrap();
    t
}

#[test]
fn the_relay_is_the_builder_s_own_never_the_game_folder_s() {
    let f = folder("relay", 1);
    // (the game folder names one: not taken; a checkout names none: nothing is sent)
    assert_eq!(status_with(&f, &env_of(&[]), "").relay, "");
    let checkout = scratch("relay-checkout");
    std::fs::create_dir_all(checkout.join("assets")).unwrap();
    std::fs::write(checkout.join("assets/addresses.ini"), "perf_reports = https://a.example/perf\n").unwrap();
    assert_eq!(tree_relay(Some(&checkout)), "", "a checkout's addresses send nothing");
    let tree = release_tree("relay", "https://ours.example/perf");
    assert_eq!(tree_relay(Some(&tree)), "https://ours.example/perf");
    let root = tree.display().to_string();
    assert_eq!(status(&f, &env_of(&[("LONGSHOT_ROOT", &root)])).relay, "https://ours.example/perf");
    for d in [f, checkout, tree] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_file_that_is_not_a_summary_the_game_writes_is_not_sent() {
    use serde_json::json;
    let mut good = json!({"v": 2, "client": "Longshot", "fps_cap": 60, "memory": {"ram_used_gb": 3.5},
        "maps": {"Kokiri": {"new": true, "frame_ms": vec![0; 13], "fine_ms": vec![0; 19], "gpu_ms": vec![0; 11],
            "phases_ms": {"any phase": 1.5}, "shown_ms": {"sum": 1, "count": 2}}},
        "samples": [{"map": "Kokiri", "x": 1, "frames": 3}]});
    assert!(schema::fits(&good));
    for bad in [
        json!({"v": 2, "home": "/home/someone"}),
        json!({"v": 2, "os": {"nested": 1}}),
        json!({"v": 2, "samples": [{"map": "x", "path": "y"}]}),
        json!({"fps_cap": 60}),
    ] {
        assert!(!schema::fits(&bad), "{bad}");
    }
    good["maps"]["Kokiri"]["frame_ms"] = json!(vec![0; 12]);
    assert!(!schema::fits(&good), "a histogram of another bucket count");
}

#[test]
fn the_schema_is_the_game_s() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = std::fs::read_to_string(root.join("gpu/vk/perf_report.h")).unwrap();
    let body = |start: &str| {
        let at = report.find(start).unwrap();
        report[at..at + report[at..].find("\n}\n").unwrap()].to_string()
    };
    let keys = |text: &str| -> std::collections::BTreeSet<String> {
        // (the names the game's tables give, phase_name(p) and the like, are code there, not keys)
        text.split("\\\"")
            .skip(1)
            .step_by(2)
            .filter(|k| !k.is_empty() && !k.contains(' '))
            .map(str::to_string)
            .collect()
    };
    let summary = keys(&body("inline std::string summary_json"));
    let mut ours: std::collections::BTreeSet<String> =
        schema::TOP.iter().chain(&schema::MEMORY).chain(&schema::MAP).map(|k| k.to_string()).collect();
    for (_, fixed) in schema::FIXED {
        ours.extend(fixed.iter().map(|k| k.to_string()));
    }
    ours.insert("Longshot".into()); // (the client's value)
    assert_eq!(summary, ours, "summary_json's keys");
    let sample: std::collections::BTreeSet<String> = schema::SAMPLE.iter().map(|k| k.to_string()).collect();
    assert_eq!(keys(&body("inline std::string sample_json")), sample);
    let header = std::fs::read_to_string(root.join("gpu/vk/perf_report_schema.h")).unwrap();
    for (name, (_, n)) in ["kFrameBuckets", "kFineBuckets", "kGpuBuckets"].iter().zip(schema::HISTOGRAMS) {
        let line = header.lines().find(|l| l.contains(&format!("constexpr double {name}[]"))).unwrap();
        assert_eq!(line.matches(',').count() + 2, n, "{name}: its bounds and +Inf");
    }
}

#[test]
fn the_status() {
    let f = folder("status", 3);
    let tree = release_tree("status", "https://relay.example/perf");
    let s = status(&f, &env_of(&[("LONGSHOT_ROOT", &tree.display().to_string())]));
    assert_eq!(
        s,
        Status {
            tick: true,
            keep_sent: true,
            waiting: 3,
            version: "v3.1.0".into(),
            release: true,
            relay: "https://relay.example/perf".into(),
            ..Status::default()
        }
    );
    let _ = std::fs::remove_dir_all(&f);
}
