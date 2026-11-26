//! The builder's windows, under `--output user`: a build's progress, an error, the crash window and its pages, and the
//! first run's picker of the game's .nup. Slint with only its winit back end and its software renderer (no Qt, no GPU:
//! a crash may have been the GPU's), two embedded fonts (Inter, and Noto Sans Black for headings), drawn in the Longshot Launcher's brand look (ui/windows.slint).
//!
//! The windows live on a thread of their own, which runs Slint's event loop (Slint's winit back end lets it run off the
//! main thread on Linux and Windows); [`Ui`] is a handle any thread calls. Nothing opens until a window is needed:
//!
//! - [`Ui::step`] shows the progress window only once a build has gone on for [`PROGRESS_AFTER`] (a quick update
//!   flashes nothing); [`Ui::done`] closes it, as soon as the game is started.
//! - [`Ui::page`], [`Ui::error`], [`Ui::crash_window`], [`Ui::pick_game`] wait for the player.
//! - A window that can't open (no display; winit fails) is [`NoWindow`]: the caller says it in text instead (and keeps
//!   a crash record for the next start).
//!
//! Keys held or mashed as a page appears press nothing: a key's repeat, and any key or click in a page's first
//! [`PAGE_GUARD`], are ignored.

use crash::flow::{Choice, Desk, Page, Pages};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The windows ui/windows.slint makes (compiled by build.rs).
#[allow(missing_docs, clippy::all)]
mod windows {
    slint::include_modules!();
}
use slint::ComponentHandle;
use windows::{PageWindow, PickWindow, ProgressWindow};

/// How long a build goes on before its progress window shows.
pub const PROGRESS_AFTER: Duration = Duration::from_millis(500);
/// A page's first moments, in which no key or click counts.
pub const PAGE_GUARD: Duration = Duration::from_millis(400);

/// A question for the player ([`Ui::confirm`]): what is asked, the facts the answer should rest on, and the two answers.
#[derive(Clone, Copy, Debug)]
pub struct Confirm<'a> {
    /// The window's title.
    pub title: &'a str,
    /// The question, a paragraph each.
    pub lines: &'a [&'a str],
    /// What the player decides by, name and value: the mod, its source, its code's hash, the permissions asked.
    pub facts: &'a [(&'a str, &'a str)],
    /// The button that says yes.
    pub accept: &'a str,
    /// The button that says no.
    pub decline: &'a str,
}

impl Confirm<'_> {
    /// The facts as the window shows them, one a line, their values aligned.
    pub fn block(&self) -> String {
        let width = self.facts.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
        self.facts
            .iter()
            .map(|(k, v)| format!("{k}{}  {v}", " ".repeat(width - k.chars().count())))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// No window could open (no display, or the windowing system refused): say it in text instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoWindow(pub String);

impl std::fmt::Display for NoWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no window could open: {}", self.0)
    }
}

impl std::error::Error for NoWindow {}

/// The windows, on the UI thread.
struct Windows {
    progress: ProgressWindow,
    page: PageWindow,
    pick: PickWindow,
}

thread_local! {
    static WINDOWS: std::cell::RefCell<Option<Windows>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&Windows) -> R) -> Option<R> {
    WINDOWS.with(|w| w.borrow().as_ref().map(f))
}

/// What Cancel in the progress window does.
type Cancel = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    /// The UI thread: started, or why it could not be.
    started: Option<Result<(), NoWindow>>,
    /// The build's first step (the progress window's clock).
    first_step: Option<Instant>,
    /// The build is done: the progress window closed, not shown again.
    done: bool,
    /// The last line said in text (when there is no window).
    said: Option<(String, u32)>,
}

/// The builder's windows: a handle, cheap to clone, any thread's.
#[derive(Clone)]
pub struct Ui {
    state: Arc<Mutex<State>>,
    cancel: Arc<Mutex<Cancel>>,
}

impl Default for Ui {
    fn default() -> Ui {
        Ui::new()
    }
}

/// Cancel's default, as Ctrl-C stops a build: the builder ends at once (its children with it: build_core's child.rs),
/// exit status 130.
fn cancel_as_ctrl_c() {
    std::process::exit(130);
}

impl Ui {
    /// The windows, none open yet.
    pub fn new() -> Ui {
        Ui { state: Arc::default(), cancel: Arc::new(Mutex::new(Arc::new(cancel_as_ctrl_c))) }
    }

    /// The windows, the UI thread started now: Err when no window can open.
    pub fn open() -> Result<Ui, NoWindow> {
        let ui = Ui::new();
        ui.start()?;
        Ok(ui)
    }

    /// What Cancel in the progress window (or closing it) does, instead of ending the builder at once.
    pub fn on_cancel(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(f);
    }

    fn start(&self) -> Result<(), NoWindow> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = &st.started {
            return r.clone();
        }
        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        let spawned = std::thread::Builder::new().name("ui".into()).spawn(move || ui_thread(tx, cancel));
        let r = match spawned {
            Ok(_) => rx.recv().unwrap_or_else(|_| Err(NoWindow("the window thread stopped".into()))),
            Err(e) => Err(NoWindow(e.to_string())),
        };
        st.started = Some(r.clone());
        r
    }

    /// Whether windows can open (the UI thread started, or starts now).
    pub fn available(&self) -> bool {
        self.start().is_ok()
    }

    /// The build's step and how far the whole build is (0 to 1). The progress window shows once the build has gone on
    /// for [`PROGRESS_AFTER`] since its first step; with no window, a line on stderr each time the step or the whole
    /// percent changes.
    pub fn step(&self, text: &str, fraction: f64) {
        let first = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.done {
                return;
            }
            let first = st.first_step.is_none();
            st.first_step.get_or_insert_with(Instant::now);
            first
        };
        if self.start().is_err() {
            self.say(text, fraction);
            return;
        }
        let (text, fraction) = (text.to_string(), fraction.clamp(0.0, 1.0) as f32);
        let state = self.state.clone();
        let _ = slint::invoke_from_event_loop(move || {
            with(|w| {
                w.progress.set_step(text.into());
                w.progress.set_fraction(fraction);
            });
            if first {
                slint::Timer::single_shot(PROGRESS_AFTER, move || {
                    if !state.lock().unwrap_or_else(|e| e.into_inner()).done {
                        with(|w| w.progress.show().ok());
                    }
                });
            }
        });
    }

    fn say(&self, text: &str, fraction: f64) {
        let pct = (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = (text.to_string(), pct);
        if st.said.as_ref() != Some(&now) {
            eprintln!("longshot: {text} ({pct} %)");
            st.said = Some(now);
        }
    }

    /// The build is done (or the game starts): the progress window closes, and does not show again.
    pub fn done(&self) {
        let started = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.done = true;
            matches!(st.started, Some(Ok(())))
        };
        if started {
            let _ = slint::invoke_from_event_loop(|| {
                with(|w| w.progress.hide().ok());
            });
        }
    }

    /// A page, until a button is pressed (its index) or the window is closed (None). The progress window hides while
    /// it shows.
    pub fn page(&self, page: &Page) -> Result<Option<usize>, NoWindow> {
        let last = page.buttons.len().saturating_sub(1);
        self.page_with_keys(page, 0, last)
    }

    /// [`Ui::page`] with the buttons Enter and Escape press (their indexes), a click being the other way to one.
    fn page_with_keys(&self, page: &Page, enter: usize, escape: usize) -> Result<Option<usize>, NoWindow> {
        self.start()?;
        let heading = {
            let t = page.title.strip_prefix("Longshot: ").unwrap_or(page.title);
            let mut c = t.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        };
        let (title, text) = (page.title.to_string(), page.text.to_string());
        let block = page.block.unwrap_or("").to_string();
        let buttons: Vec<slint::SharedString> = page.buttons.iter().map(|b| (*b).into()).collect();
        let n = buttons.len();
        let (tx, rx) = mpsc::channel::<Option<usize>>();
        let r = slint::invoke_from_event_loop(move || {
            with(|w| {
                let _ = w.progress.hide();
                let p = &w.page;
                p.set_window_title(title.into());
                p.set_heading(heading.into());
                p.set_text(text.into());
                p.set_block(block.into());
                p.set_buttons(std::rc::Rc::new(slint::VecModel::from(buttons)).into());
                let shown = Instant::now();
                let ready = move || shown.elapsed() >= PAGE_GUARD;
                let (t1, t2, t3) = (tx.clone(), tx.clone(), tx);
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                let (a1, a2, a3) = (answered.clone(), answered.clone(), answered);
                let weak = p.as_weak();
                let finish = move |weak: &slint::Weak<PageWindow>,
                                   tx: &mpsc::Sender<Option<usize>>,
                                   a: &std::cell::Cell<bool>,
                                   hit| {
                    if !a.replace(true) {
                        let _ = tx.send(hit);
                        if let Some(p) = weak.upgrade() {
                            let _ = p.hide();
                        }
                    }
                };
                let (w1, w2) = (weak.clone(), weak);
                p.on_pressed(move |i| {
                    if ready() {
                        finish(&w1, &t1, &a1, Some(i as usize));
                    }
                });
                p.on_key(move |k, repeat| {
                    if !repeat && ready() && n > 0 {
                        finish(&w2, &t2, &a2, Some(if k == 0 { enter } else { escape }));
                    }
                });
                p.window().on_close_requested(move || {
                    if !a3.replace(true) {
                        let _ = t3.send(None);
                    }
                    slint::CloseRequestResponse::HideWindow
                });
                let _ = p.show();
            });
        });
        r.map_err(|e| NoWindow(e.to_string()))?;
        let hit = rx.recv().map_err(|_| NoWindow("the window thread stopped".into()))?;
        Ok(hit)
    }

    /// A question the player answers in a window of the builder's own: true only when the player pressed the accept
    /// button (a click: Enter, Escape, closing the window and a key in the first [`PAGE_GUARD`] all decline). The one
    /// way anything the game asks for, such as trusting a mod's code or granting a permission, is granted: the game only
    /// asks, it never answers.
    pub fn confirm(&self, c: &Confirm) -> Result<bool, NoWindow> {
        let text = c.lines.join("\n\n");
        let block = c.block();
        let hit = self.page_with_keys(
            &Page {
                title: c.title,
                text: &text,
                block: (!block.is_empty()).then_some(block.as_str()),
                buttons: &[c.accept, c.decline],
            },
            1,
            1,
        )?;
        Ok(hit == Some(0))
    }

    /// An error, until the player closes it (Close).
    pub fn error(&self, text: &str) -> Result<(), NoWindow> {
        self.page(&Page { title: "Longshot: the game could not be built", text, block: None, buttons: &["Close"] })
            .map(|_| ())
    }

    /// The crash window and its pages ([`crash::flow::show`], with the desktop's clipboard, browser and curl): how it
    /// ended.
    pub fn crash_window(&self, shown: &crash::Shown) -> Result<Choice, NoWindow> {
        self.start()?;
        let mut pages = UiPages { ui: self, failed: None };
        let choice = crash::flow::show(shown, &mut pages, &mut Desk::system());
        match pages.failed {
            Some(e) => Err(e),
            None => Ok(choice),
        }
    }

    /// The first run's picker: the player's .nup (or a dump folder), by its path or the system's file chooser. None
    /// when the player cancelled.
    pub fn pick_game(&self) -> Result<Option<PathBuf>, NoWindow> {
        self.start()?;
        let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
        let r = slint::invoke_from_event_loop(move || {
            with(|w| {
                let _ = w.progress.hide();
                let p = &w.pick;
                p.set_note("".into());
                let shown = Instant::now();
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                let (a1, a2) = (answered.clone(), answered);
                let (t1, t2) = (tx.clone(), tx);
                let weak = p.as_weak();
                let w1 = weak.clone();
                p.on_done(move |ok| {
                    let Some(p) = w1.upgrade() else { return };
                    if shown.elapsed() < PAGE_GUARD || a1.get() {
                        return;
                    }
                    if !ok {
                        a1.set(true);
                        let _ = t1.send(None);
                        let _ = p.hide();
                        return;
                    }
                    let path = PathBuf::from(p.get_path().trim().trim_matches('"'));
                    if path.as_os_str().is_empty() || !path.exists() {
                        p.set_note("There is no such file. Paste the path of your .nup, or press Browse.".into());
                        return;
                    }
                    a1.set(true);
                    let _ = t1.send(Some(path));
                    let _ = p.hide();
                });
                let w2 = weak;
                p.on_browse(move || {
                    let w = w2.clone();
                    std::thread::spawn(move || {
                        let file = rfd::FileDialog::new()
                            .set_title("Pick your Ocarina of Time .nup")
                            .add_filter("Ocarina of Time (Nintendo Switch 2)", &["nup"])
                            .pick_file();
                        if let Some(f) = file {
                            let _ = w.upgrade_in_event_loop(move |p| p.set_path(f.display().to_string().into()));
                        }
                    });
                });
                p.window().on_close_requested(move || {
                    if !a2.replace(true) {
                        let _ = t2.send(None);
                    }
                    slint::CloseRequestResponse::HideWindow
                });
                let _ = p.show();
            });
        });
        r.map_err(|e| NoWindow(e.to_string()))?;
        rx.recv().map_err(|_| NoWindow("the window thread stopped".into()))
    }
}

/// [`Pages`] on the builder's window; a window that can't open ends the flow (no button: Close).
pub struct UiPages<'a> {
    ui: &'a Ui,
    /// Why no window could open, when it could not.
    pub failed: Option<NoWindow>,
}

impl<'a> UiPages<'a> {
    /// Pages on `ui`.
    pub fn new(ui: &'a Ui) -> UiPages<'a> {
        UiPages { ui, failed: None }
    }
}

impl Pages for UiPages<'_> {
    fn page(&mut self, page: &Page) -> Option<usize> {
        if self.failed.is_some() {
            return None;
        }
        match self.ui.page(page) {
            Ok(hit) => hit,
            Err(e) => {
                self.failed = Some(e);
                None
            }
        }
    }
}

/// The UI thread: Slint's winit back end with its software renderer, the windows made (hidden), then its event loop
/// for as long as the builder runs (winit makes one event loop a process).
fn ui_thread(ready: mpsc::Sender<Result<(), NoWindow>>, cancel: Arc<Mutex<Cancel>>) {
    let made = (|| -> Result<Windows, NoWindow> {
        let no = |e: slint::PlatformError| NoWindow(e.to_string());
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .map_err(no)?;
        let w = Windows {
            progress: ProgressWindow::new().map_err(no)?,
            page: PageWindow::new().map_err(no)?,
            pick: PickWindow::new().map_err(no)?,
        };
        let c = cancel.clone();
        w.progress.on_cancel(move || {
            let f = c.lock().unwrap_or_else(|e| e.into_inner()).clone();
            f();
        });
        w.progress.window().on_close_requested(move || {
            let f = cancel.lock().unwrap_or_else(|e| e.into_inner()).clone();
            f();
            slint::CloseRequestResponse::HideWindow
        });
        Ok(w)
    })();
    match made {
        Ok(w) => {
            WINDOWS.with(|cell| *cell.borrow_mut() = Some(w));
            let _ = ready.send(Ok(()));
            let _ = slint::run_event_loop_until_quit();
        }
        Err(e) => {
            let _ = ready.send(Err(e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_facts_are_one_a_line_with_their_values_aligned() {
        let c = Confirm {
            title: "Longshot: trust a mod's code?",
            lines: &["Trust this mod's code?"],
            facts: &[("Mod", "ann.fast-travel"), ("Source", "https://mods.example/repo"), ("Code hash", "9f2c")],
            accept: "Trust",
            decline: "Don't trust",
        };
        assert_eq!(c.block(), "Mod        ann.fast-travel\nSource     https://mods.example/repo\nCode hash  9f2c");
        assert_eq!(Confirm { facts: &[], ..c }.block(), "");
    }
}
