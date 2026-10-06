//! Ctrl-C handling. The first press stops the current turn; a second press
//! before the turn notices quits the program.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install() {
    let _ = ctrlc::set_handler(|| {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            restore_terminal();
            eprint!("\x1b[?2004l\x1b[?25h\nbye\n");
            std::process::exit(130);
        }
    });
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn reset() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}

/// Marks the current turn as interrupted (e.g. Ctrl-C at an approval prompt).
pub fn trip() {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

static DRAFT: std::sync::Mutex<Option<crate::input::Draft>> = std::sync::Mutex::new(None);

/// Only the interactive REPL owns a composer. Headless callers never read stdin.
pub struct BusyInput(bool);
impl Drop for BusyInput {
    fn drop(&mut self) {
        if self.0 {
            poll_keys();
            if let Ok(mut draft) = DRAFT.lock() {
                if let Some(d) = draft.as_mut() {
                    d.active = false;
                }
            }
            restore_terminal();
            print!("\x1b[?2004l\x1b[?25h");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }
}

fn raw_busy() {
    if let Some(Some(cooked)) = COOKED.get() {
        let mut t = *cooked;
        t.c_lflag &= !(libc::ICANON | libc::ECHO);
        t.c_lflag |= libc::NOFLSH | libc::ISIG;
        t.c_iflag &= !(libc::ICRNL | libc::INLCR | libc::IGNCR | libc::IXON);
        // ISIG stays enabled: Ctrl-C still interrupts child commands.
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
        print!("\x1b[?2004h\x1b[?25h");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

pub fn begin_busy(suggestions: Vec<String>) -> BusyInput {
    let enabled = crate::ui::tty() && crate::ui::height() >= 2 && unsafe { libc::isatty(0) == 1 };
    if enabled {
        if let Ok(mut draft) = DRAFT.lock() {
            let mut d = crate::input::Draft::default();
            d.active = true;
            d.suggestions = suggestions;
            *draft = Some(d);
        }
        raw_busy();
    }
    BusyInput(enabled)
}

/// Approval editors have exclusive ownership of stdin. A busy draft is kept in
/// memory and cannot be used as an answer to the approval question.
pub struct SuspendedInput(bool);
impl Drop for SuspendedInput {
    fn drop(&mut self) {
        if self.0 {
            if let Ok(mut draft) = DRAFT.lock() {
                if let Some(d) = draft.as_mut() {
                    d.active = true;
                }
            }
            raw_busy();
        }
    }
}
pub fn suspend_input() -> SuspendedInput {
    poll_keys();
    let active = DRAFT
        .lock()
        .ok()
        .and_then(|mut draft| {
            draft.as_mut().map(|d| {
                let active = d.active;
                d.active = false;
                active
            })
        })
        .unwrap_or(false);
    if active {
        restore_terminal();
        print!("\x1b[?2004l\x1b[?25h");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    SuspendedInput(active)
}

/// Polling fits the existing model/tool event loops; no second render thread.
pub fn poll_keys() {
    let Ok(mut draft) = DRAFT.lock() else {
        return;
    };
    let Some(d) = draft.as_mut().filter(|d| d.active && !d.submitted) else {
        return;
    };
    if d.escape_expired() {
        trip();
    }
    unsafe {
        let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        if libc::poll(&mut p, 1, 0) <= 0 || p.revents & libc::POLLIN == 0 {
            return;
        }
        let mut buf = [0u8; 4096];
        let n = libc::read(0, buf.as_mut_ptr().cast(), buf.len());
        if n > 0 && d.feed(&buf[..n as usize]) {
            trip();
        }
    }
}

pub fn take_draft() -> Option<crate::input::Draft> {
    DRAFT.lock().ok()?.take().filter(|d| !d.text.is_empty())
}
pub fn composer(cells: usize) -> Option<(String, usize, String)> {
    DRAFT.lock().ok()?.as_ref().filter(|d| d.active).map(|d| d.preview(cells))
}

/// The terminal mode at startup (cooked, with Ctrl-C generating SIGINT).
static COOKED: OnceLock<Option<libc::termios>> = OnceLock::new();

/// Remembers the terminal's normal mode. Call once at startup.
pub fn save_terminal() {
    COOKED.get_or_init(|| unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        (libc::isatty(0) == 1 && libc::tcgetattr(0, &mut t) == 0).then_some(t)
    });
}

/// Puts the terminal back in its normal mode before a turn runs, so Ctrl-C
/// raises SIGINT even if the line editor left raw mode behind.
pub fn restore_terminal() {
    if let Some(Some(t)) = COOKED.get() {
        unsafe {
            let mut normal = *t;
            // Some PTY launchers start with ISIG off. Plain-output turns have
            // no key composer, so Ctrl-C must still reach the signal handler.
            normal.c_lflag |= libc::ISIG;
            libc::tcsetattr(0, libc::TCSANOW, &normal);
        }
    }
}

/// A bounded draft must never silently submit a truncated paste.
pub fn draft_limited() -> bool {
    DRAFT.lock().ok().and_then(|draft| draft.as_ref().map(|d| d.truncated)).unwrap_or(false)
}
