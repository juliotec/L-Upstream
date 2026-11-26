//! The crash window's pages, in turn, as the game's crash helper had them: the window ("Longshot closed unexpectedly",
//! when, why) with Report automatically, Report manually, Open the log folder, Save diagnostics and Close; Report
//! automatically's page with the report as it is sent (Show all of it, Send, Cancel) and its answer; Report manually's
//! box. The pages themselves are a [`Pages`]' (the builder's window: the `ui` crate), so this runs with none in tests.

use crate::names::{self, Copied};
use crate::report::Shown;
use crate::{desk, diag, post, store};

/// The user agent the crash reports are posted with.
pub const AGENT: &str = "longshot-crash-report";

/// A page: its title, its text, a block shown as it is below the text (the report), and its buttons (the first is
/// Enter's, the last Escape's).
#[derive(Clone, Copy, Debug)]
pub struct Page<'a> {
    /// The window's title.
    pub title: &'a str,
    /// The page's text.
    pub text: &'a str,
    /// Text shown as it is, in a box of its own below.
    pub block: Option<&'a str>,
    /// The buttons, left to right.
    pub buttons: &'a [&'a str],
}

/// What shows pages: the button pressed, by its index; None when the window was closed otherwise.
pub trait Pages {
    /// Shows `page` until a button is pressed or the window is closed.
    fn page(&mut self, page: &Page) -> Option<usize>;
}

/// Posts a payload file to an address: the HTTP status and the body.
pub type PostFn = dyn FnMut(&std::path::Path, &str) -> (u16, String);

/// What the desktop does for the window: the clipboard, opening things, and posting. The real ones by default
/// ([`Desk::system`]); tests stand in.
pub struct Desk {
    /// Puts text on the clipboard.
    pub clipboard: Box<dyn FnMut(&str) -> Copied>,
    /// Opens a link, a folder or a file.
    pub open: Box<dyn FnMut(&str) -> bool>,
    /// Posts the payload file to the address: the status and the body.
    pub post: Box<PostFn>,
}

impl Desk {
    /// The system's.
    pub fn system() -> Desk {
        Desk {
            clipboard: Box::new(desk::clipboard),
            open: Box::new(desk::open),
            post: Box::new(|p, a| post::post(p, a, AGENT)),
        }
    }
}

/// How the crash window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Closed.
    Close,
    /// The report was sent (Report automatically, or with no window: `send=1`).
    Sent,
    /// Report manually made its report.
    Reported,
}

fn payload_path(s: &Shown) -> Option<std::path::PathBuf> {
    Some(s.paths.pending().join(format!("sending-{}.json", s.record_name.as_deref()?)))
}

/// Report automatically with no window, as the player chose in the launcher (`send=1` in longshot_crash.ini): the
/// same report, posted the same way, its answer kept and a line in the run's log. Whether it was sent (when not, the
/// window shows as it would have).
pub fn send_quietly(s: &Shown, desk: &mut Desk) -> bool {
    let wanted = store::read_text(&s.paths.crash_ini, 4096).is_some_and(|t| names::auto_send(&t));
    if !wanted || !s.send_offered() {
        return false;
    }
    let (Some(payload), Some(address), Some(record)) =
        (payload_path(s), s.address.as_deref(), s.record_name.as_deref())
    else {
        return false;
    };
    let Ok((json, _)) = s.build_report() else { return false };
    if std::fs::write(&payload, &json).is_err() {
        return false;
    }
    let (status, body) = (desk.post)(&payload, address);
    let (sent, result, _) = names::answer(status, &body);
    if sent {
        store::keep_answer(&s.paths, record, status, Some(&json));
    }
    if let Some(log) = &s.log.path {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log) {
            let _ = writeln!(
                f,
                "[builder crash] sent on its own (longshot_crash.ini): {}",
                result.replace(['\n', '\r'], " ")
            );
        }
    }
    let _ = std::fs::remove_file(&payload);
    sent
}

/// Report automatically's page: the report as it goes, Show all of it, Send; the answer. Whether it was sent.
fn send_report(s: &Shown, ui: &mut dyn Pages, desk: &mut Desk) -> bool {
    const TITLE: &str = "Longshot: send to the team";
    let made = match (payload_path(s), s.build_report()) {
        (Some(p), Ok((json, shown))) if std::fs::write(&p, &json).is_ok() => Ok((p, shown)),
        (_, Err(why)) => Err(why),
        _ => Err("It could not be saved in bug-reports."),
    };
    let (payload, shown) = match made {
        Ok(m) => m,
        Err(why) => {
            let text = format!("The report could not be made, so nothing was sent.\n\n{why}");
            ui.page(&Page { title: TITLE, text: &text, block: None, buttons: &["Close"] });
            return false;
        }
    };
    let mut sent = false;
    loop {
        let hit = ui.page(&Page {
            title: TITLE,
            text: "This is what goes to the Longshot team, and nothing else; only the team can read it. Show all of it opens \
                   the report as it is sent, every line of it.",
            block: Some(&shown),
            buttons: &["Send", "Show all of it", "Cancel"],
        });
        match hit {
            Some(1) => {
                (desk.open)(&payload.display().to_string());
                continue;
            }
            Some(0) => {}
            _ => break,
        }
        let (status, body) = (desk.post)(&payload, s.address.as_deref().unwrap_or(""));
        let (ok, result, again) = names::answer(status, &body);
        if ok {
            sent = true;
            if let Some(r) = s.record_name.as_deref() {
                store::keep_answer(&s.paths, r, status, None);
            }
            ui.page(&Page { title: "Longshot: sent", text: &result, block: None, buttons: &["Done"] });
            break;
        }
        let buttons: &[&str] = if again { &["Try again", "Close"] } else { &["Close"] };
        if ui.page(&Page { title: "Longshot: not sent", text: &result, block: None, buttons }) != Some(0) || !again {
            break;
        }
    }
    let _ = std::fs::remove_file(&payload);
    sent
}

/// Report manually: the report on the clipboard, saved as `pending/report-<UTC>.txt`, the issue form opened; the box
/// that says so (Open the report, Done).
fn report(s: &Shown, ui: &mut dyn Pages, desk: &mut Desk) {
    let now = store::now();
    let card = s.card();
    let text = s.manual_text(&card, now);
    let path = s.paths.pending().join(format!("report-{}.txt", names::utc(now)));
    let saved = std::fs::create_dir_all(s.paths.pending()).is_ok() && std::fs::write(&path, &text).is_ok();
    if saved {
        store::prune_reports(&s.paths);
    }
    let copied = (desk.clipboard)(&text);
    let opened = (desk.open)(&s.issue_url(&card));
    let local = path.display().to_string();
    let m = names::ready_text(copied, opened, saved.then_some(local.as_str()));
    let buttons: &[&str] = if saved { &["Open the report", "Done"] } else { &["Done"] };
    for shown in 0.. {
        let hit = ui.page(&Page { title: "Longshot: report ready", text: &m, block: None, buttons });
        if hit.is_none() && shown == 0 && saved {
            (desk.open)(&local);
        }
        if !saved || hit != Some(0) {
            break;
        }
        (desk.open)(&local);
    }
}

/// The diagnostics bundle saved in `bug-reports/` and its folder opened: the box that says where.
fn save_diagnostics(s: &Shown, ui: &mut dyn Pages, desk: &mut Desk) {
    let out = s.paths.reports.join(format!("diagnostics-{}.zip", names::utc(store::now())));
    let text = match diag::write(&s.paths.diagnostics(s.log.path.as_deref()), &out, &s.who) {
        Ok(()) => {
            (desk.open)(&s.paths.reports.display().to_string());
            format!(
                "The diagnostics are saved as {}: the logs (your home folder and user name taken out), the build's record, \
                 the profile's settings (never your saves) and your PC's system. Nothing was sent: attach it to an issue.",
                out.display()
            )
        }
        Err(e) => format!("The diagnostics could not be saved: {e}."),
    };
    ui.page(&Page { title: "Longshot: diagnostics", text: &text, block: None, buttons: &["Done"] });
}

/// The crash window, page after page, until it is closed: how it ended. `send=1` sends with no window first; a report
/// that can't be sent then shows the window as always.
pub fn show(s: &Shown, ui: &mut dyn Pages, desk: &mut Desk) -> Choice {
    if send_quietly(s, desk) {
        return Choice::Sent;
    }
    let mut choice = Choice::Close;
    loop {
        let offered = s.send_offered();
        let message = crate::facts::message(s.verdict, s.stage, s.cause, offered);
        let all = ["Report automatically", "Report manually", "Open the log folder", "Save diagnostics", "Close"];
        let buttons = if offered { &all[..] } else { &all[1..] };
        let hit = ui.page(&Page { title: crate::facts::title(s.verdict), text: &message, block: None, buttons });
        match hit.map(|h| buttons[h]) {
            Some("Report automatically") => {
                if send_report(s, ui, desk) {
                    choice = Choice::Sent;
                }
            }
            Some("Open the log folder") => {
                (desk.open)(&s.paths.log_folder(s.log.path.as_deref()).display().to_string());
            }
            Some("Save diagnostics") => save_diagnostics(s, ui, desk),
            Some("Report manually") => {
                report(s, ui, desk);
                return Choice::Reported;
            }
            _ => return choice,
        }
    }
}
