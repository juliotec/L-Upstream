//! After a run, with no game and no window: the records a game leaves (a crash record, a marker made into an unclean
//! record), the window's pages through a stand-in, Report automatically's post through a stand-in for curl, `send=1`,
//! and a record nobody showed at the next start (as tests/test_crash_dialog.py ran the C helper).

#![allow(missing_docs)]

use crash::flow::{self, Choice, Desk, Page, Pages};
use crash::{Exit, Paths, Run, Verdict, names, store};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const RUN: &str = "0123456789abcdef0123456789abcdef";

fn folder(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("crash-after-run").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("bug-reports/pending")).unwrap();
    std::fs::create_dir_all(d.join("bug-reports/running")).unwrap();
    std::fs::create_dir_all(d.join("logs")).unwrap();
    d
}

fn log(game: &Path, extra: &[&str]) {
    let mut lines = vec![
        "[hos host] longshot v3.1.0 (source 1234abcd)".to_string(),
        format!("[hos host] run_id: {RUN}"),
        "[hos host] report: os TestOS".into(),
        "[hos host] report: cpu TestCPU (8 threads), 16 GB".into(),
        "[hos Main] map: Kokiri Forest".into(),
    ];
    lines.extend(extra.iter().map(|s| s.to_string()));
    std::fs::write(game.join("logs/longshot_log.txt"), lines.join("\n") + "\n").unwrap();
}

fn crash_record(game: &Path, pid: u32, started: i64) -> String {
    let name = format!("crash-{}.txt", names::utc(started + 5));
    let text = format!(
        "Longshot crash record 1\nversion: v3.1.0\nrun_id: {RUN}\nwhen: {}\nuptime_s: 5\nreason: unhandled exception\n\
         module_offset: longshot+0x10\npid: {pid}\nstarted: {}\nexception: 0xc0000005\nrecord_complete: yes\n",
        names::utc(started + 5),
        names::utc(started)
    );
    std::fs::write(game.join("bug-reports/pending").join(&name), text).unwrap();
    name
}

/// A marker as hos_crash_record.c's Session writes it.
fn marker(game: &Path, pid: u32, started: i64, quit: bool) {
    let mut b = vec![0u8; 4312];
    b[..19].copy_from_slice(b"Longshot running 1\0");
    b[24..32].copy_from_slice(&u64::from(pid).to_le_bytes());
    b[32..40].copy_from_slice(&(started as u64).to_le_bytes());
    b[52..84].copy_from_slice(RUN.as_bytes());
    b[85..91].copy_from_slice(b"v3.1.0");
    let log = game.join("logs/longshot_log.txt").display().to_string();
    b[214..214 + log.len()].copy_from_slice(log.as_bytes());
    if quit {
        b.extend_from_slice(b"quit\n");
    }
    std::fs::write(game.join(format!("bug-reports/running/{pid}.running")), b).unwrap();
}

/// Presses the buttons named, in turn (None: closes the window); records each page's title.
struct Script {
    press: Vec<Option<&'static str>>,
    seen: Rc<RefCell<Vec<String>>>,
}

impl Pages for Script {
    fn page(&mut self, p: &Page) -> Option<usize> {
        self.seen.borrow_mut().push(format!("{}|{}", p.title, p.buttons.join(",")));
        let want = if self.press.is_empty() { None } else { self.press.remove(0) }?;
        Some(p.buttons.iter().position(|b| *b == want).unwrap_or_else(|| panic!("no {want} in {:?}", p.buttons)))
    }
}

fn desk(posted: Rc<RefCell<Vec<String>>>, status: u16) -> Desk {
    Desk {
        clipboard: Box::new(|_| names::Copied::Kept),
        open: Box::new(|_| true),
        post: Box::new(move |p, a| {
            posted.borrow_mut().push(format!("{a}\n{}", std::fs::read_to_string(p).unwrap()));
            (status, "{}".into())
        }),
    }
}

#[test]
fn a_crash_is_shown_reported_and_sent_once() {
    let game = folder("crash");
    let pid = 4242;
    let started = store::now() - 60;
    log(
        &game,
        &[
            "[hos Main] CRASH: exception 0xc0000005 (read 0x8) at /home/someone/longshot+0x10",
            "[hos Main] CRASH:   x0 0000000000000001",
        ],
    );
    std::fs::write(
        game.join("longshot_version.txt"),
        "source=1234abcd\ncrash_reports=https://d.example/telemetry/crash\n",
    )
    .unwrap();
    let name = crash_record(&game, pid, started);
    let paths = Paths::of_game(&game);
    let s =
        crash::after_run(&paths, &Run { pid, started, exit: Exit { code: Some(139) }, oom: false }).expect("a crash");
    assert_eq!((s.verdict, s.record_name.as_deref()), (Verdict::Crash, Some(name.as_str())));
    assert!(s.send_offered());
    let (json, shown) = s.build_report().unwrap();
    assert!(
        json.contains("\"client\": \"Longshot\"")
            && json.contains("\"release\": true")
            && json.contains("at longshot+0x10")
    );
    assert!(!json.contains("x0 0000") && !json.contains("someone"), "no memory rows, no home: {json}");
    assert!(shown.contains("Map: Kokiri Forest"));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let posted = Rc::new(RefCell::new(Vec::new()));
    let mut ui = Script {
        press: vec![Some("Report automatically"), Some("Send"), Some("Done"), Some("Close")],
        seen: seen.clone(),
    };
    assert_eq!(flow::show(&s, &mut ui, &mut desk(posted.clone(), 201)), Choice::Sent);
    assert_eq!(posted.borrow().len(), 1);
    assert!(posted.borrow()[0].starts_with("https://d.example/telemetry/crash\n{"));
    assert!(store::sent_already(&paths, &name), "its answer kept");
    assert!(
        seen.borrow()[3].starts_with("Longshot closed unexpectedly|Report manually"),
        "not offered again: {:?}",
        seen.borrow()
    );
    crash::mark_shown(&s);
    assert!(crash::at_start(&paths).is_none(), "shown: not again at the next start");
}

#[test]
fn send_1_sends_with_no_window() {
    let game = folder("send1");
    let started = store::now() - 60;
    log(&game, &["[hos Main] CRASH: exception 0xc0000005 (read 0x8) at longshot+0x10"]);
    std::fs::write(game.join("longshot_version.txt"), "crash_reports=https://d.example/c\n").unwrap();
    std::fs::write(game.join("longshot_crash.ini"), "send=1\n").unwrap();
    let name = crash_record(&game, 7, started);
    let paths = Paths::of_game(&game);
    let s = crash::after_run(&paths, &Run { pid: 7, started, exit: Exit::default(), oom: false }).unwrap();
    let posted = Rc::new(RefCell::new(Vec::new()));
    let mut ui = Script { press: vec![], seen: Rc::new(RefCell::new(Vec::new())) };
    assert_eq!(flow::show(&s, &mut ui, &mut desk(posted.clone(), 201)), Choice::Sent);
    assert!(ui.seen.borrow().is_empty(), "no window");
    assert!(game.join("bug-reports/sent").join(name.replace(".txt", ".json")).is_file(), "a copy of what went");
    assert!(
        std::fs::read_to_string(game.join("logs/longshot_log.txt"))
            .unwrap()
            .contains("sent on its own (longshot_crash.ini): Sent.")
    );
    // (a report that can't be sent shows the window after all)
    let game = folder("send1-fails");
    log(&game, &["[hos Main] CRASH: exception 0xc0000005 (read 0x8) at longshot+0x10"]);
    std::fs::write(game.join("longshot_version.txt"), "crash_reports=https://d.example/c\n").unwrap();
    std::fs::write(game.join("longshot_crash.ini"), "send=1\n").unwrap();
    crash_record(&game, 7, started);
    let s =
        crash::after_run(&Paths::of_game(&game), &Run { pid: 7, started, exit: Exit::default(), oom: false }).unwrap();
    let mut ui = Script { press: vec![None], seen: Rc::new(RefCell::new(Vec::new())) };
    flow::show(&s, &mut ui, &mut desk(Rc::new(RefCell::new(Vec::new())), 0));
    assert_eq!(ui.seen.borrow().len(), 1, "the window");
}

#[test]
fn a_marker_left_is_an_unclean_end_and_a_quit_is_none() {
    let game = folder("marker");
    let started = store::now() - 60;
    log(&game, &[]);
    marker(&game, 99, started, false);
    let paths = Paths::of_game(&game);
    let s = crash::after_run(&paths, &Run { pid: 99, started, exit: Exit { code: None }, oom: true })
        .expect("an unclean end");
    assert_eq!((s.verdict, s.cause), (Verdict::Unclean, crash::Cause::Memory));
    let text = s.record.as_deref().unwrap();
    assert!(
        text.starts_with("Longshot unclean record 1\n")
            && text.contains("oom_kill: yes")
            && text.contains(&format!("run_id: {RUN}"))
    );
    assert!(!game.join("bug-reports/running/99.running").exists(), "the marker made into its record");
    assert!(!s.is_crash_record() && !s.send_offered(), "an unclean end has no crash to send");

    let game = folder("quit");
    log(&game, &[]);
    marker(&game, 98, started, true);
    let paths = Paths::of_game(&game);
    assert!(
        crash::after_run(&paths, &Run { pid: 98, started, exit: Exit { code: Some(1) }, oom: false }).is_none(),
        "a quit is no crash"
    );
    let ledger = std::fs::read_to_string(paths.ledger()).unwrap();
    assert!(ledger.starts_with("unclean-"), "said nothing: listed as shown");
}

#[test]
fn a_normal_exit_says_nothing() {
    let game = folder("quiet");
    let started = store::now() - 60;
    let paths = Paths::of_game(&game);
    marker(&game, 11, started, false);
    assert!(crash::after_run(&paths, &Run { pid: 11, started, exit: Exit { code: Some(0) }, oom: false }).is_none());
    // (a start that failed and ended cleanly: no record, its log says why)
    std::fs::write(
        game.join("logs/longshot_log.txt"),
        format!("[hos host] longshot v3.1.0\n[hos host] run_id: {RUN}\ncannot open /g/image.bin\n"),
    )
    .unwrap();
    let s = crash::after_run(&paths, &Run { pid: 13, started, exit: Exit { code: Some(1) }, oom: false }).unwrap();
    assert_eq!((s.verdict, s.cause), (Verdict::Start, crash::Cause::Files));
}

#[test]
fn a_crash_nobody_showed_is_shown_at_the_next_start() {
    let game = folder("at-start");
    let paths = Paths::of_game(&game);
    log(&game, &["[hos Main] CRASH: exception 0xc0000005 (read 0x8) at longshot+0x10"]);
    let old = crash_record(&game, 5, store::now() - 7200);
    assert!(crash::at_start(&paths).is_none(), "the first start with a ledger: what was pending is not news");
    assert!(store::ledger_add(&paths, None, false));
    let _ = std::fs::remove_file(paths.pending().join(&old));
    let name = crash_record(&game, 6, store::now() - 60);
    let s = crash::at_start(&paths).expect("shown at the next start");
    assert_eq!(s.record_name.as_deref(), Some(name.as_str()));
    assert!(s.text().contains(&name));
    crash::mark_shown(&s);
    assert!(crash::at_start(&paths).is_none());
}

#[test]
fn report_manually_saves_and_says_so() {
    let game = folder("manual");
    let started = store::now() - 60;
    log(&game, &["[hos Main] CRASH: exception 0xc0000005 (read 0x8) at longshot+0x10"]);
    std::fs::write(game.join("longshot_options.ini"), "ultrawide=1\nkey_a=30\nmicrophone_device=My Mic\n").unwrap();
    std::fs::write(game.join("longshot_defaults.ini"), "ultrawide=0\n").unwrap();
    crash_record(&game, 8, started);
    let paths = Paths::of_game(&game);
    let s = crash::after_run(&paths, &Run { pid: 8, started, exit: Exit::default(), oom: false }).unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut ui = Script { press: vec![Some("Report manually"), Some("Done")], seen: seen.clone() };
    assert_eq!(flow::show(&s, &mut ui, &mut desk(Rc::new(RefCell::new(Vec::new())), 0)), Choice::Reported);
    let saved: Vec<String> = store::names(&paths.pending()).into_iter().filter(|n| n.starts_with("report-")).collect();
    assert_eq!(saved.len(), 1);
    let text = std::fs::read_to_string(paths.pending().join(&saved[0])).unwrap();
    assert!(
        text.contains("Version: v3.1.0")
            && text.contains("ultrawide=1")
            && !text.contains("key_a")
            && !text.contains("My Mic")
    );
    assert!(seen.borrow()[1].starts_with("Longshot: report ready|Open the report,Done"));
    let diag = Rc::new(RefCell::new(Vec::new()));
    let mut ui = Script { press: vec![Some("Save diagnostics"), Some("Done"), None], seen: diag };
    flow::show(&s, &mut ui, &mut desk(Rc::new(RefCell::new(Vec::new())), 0));
    assert!(store::names(&paths.reports).iter().any(|n| n.starts_with("diagnostics-") && n.ends_with(".zip")));
}
