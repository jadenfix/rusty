//! Local suggestions and editing. Drafts are owned by the user until Enter.
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Cmd, ConditionalEventHandler, Context, Event, EventContext, Helper, Movement, RepeatCount};
use std::borrow::Cow;
use std::path::Path;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{skills, ui};

pub const COMMANDS: &[&str] = &[
    "/help",
    "/?",
    "/exit",
    "/quit",
    "/q",
    "/clear",
    "/new",
    "/model",
    "/models",
    "/tools",
    "/mode",
    "/compact",
    "/context",
    "/tokens",
    "/usage",
    "/cost",
    "/view",
    "/adhd",
    "/verbose",
    "/theme",
    "/font",
    "/agents",
    "/subagents",
    "/swarm",
    "/tips",
    "/target",
    "/changes",
    "/audit",
    "/settings",
    "/permissions",
    "/perms",
    "/yolo",
    "/plan",
    "/goal",
    "/loop",
    "/memory",
    "/mem",
    "/remember",
    "/forget",
    "/skills",
    "/suggestions",
];

pub struct PromptHelper {
    pub enabled: bool,
    commands: Vec<String>,
    files: FilenameCompleter,
    history: HistoryHinter,
}

impl PromptHelper {
    pub fn new(cwd: &Path) -> Self {
        let mut commands: Vec<_> = COMMANDS.iter().map(|s| s.to_string()).collect();
        commands.extend(skills::all(cwd).into_iter().map(|s| format!("/{}", s.name)));
        commands.sort();
        commands.dedup();
        Self { enabled: true, commands, files: FilenameCompleter::new(), history: HistoryHinter::new() }
    }

    pub fn choices(&self, line: &str) -> Vec<String> {
        if !line.starts_with('/') {
            return Vec::new();
        }
        let (command, prefix) = line.split_once(' ').unwrap_or((line, ""));
        let values: Vec<String> = if !line.contains(' ') {
            return self.commands.iter().filter(|s| s.starts_with(line)).cloned().collect();
        } else {
            match command {
                "/mode" => vec!["auto", "careful", "standard", "vibe"],
                "/view" => vec!["default", "verbose", "adhd"],
                "/tools" => vec!["local", "daytona"],
                "/agents" | "/subagents" => vec!["off", "sub", "swarm", "auto", "default", "model"],
                "/swarm" => vec!["on", "off", "size", "models", "spread"],
                "/permissions" | "/perms" => {
                    vec!["read-only", "ask", "auto", "yolo", "allow", "deny", "remove", "reset", "check"]
                }
                "/goal" => vec!["status", "resume", "clear"],
                "/memory" | "/mem" => vec!["list", "search", "forget", "status", "helpful", "harmful"],
                "/tips" | "/suggestions" => vec!["on", "off"],
                "/theme" => {
                    return ui::THEMES
                        .iter()
                        .filter(|t| t.name.starts_with(prefix))
                        .map(|t| format!("{command} {}", t.name))
                        .collect()
                }
                "/font" => {
                    return ui::FONTS
                        .iter()
                        .filter(|t| t.starts_with(prefix))
                        .map(|t| format!("{command} {t}"))
                        .collect()
                }
                _ => Vec::new(),
            }
            .into_iter()
            .map(str::to_string)
            .collect()
        };
        values.into_iter().filter(|v| v.starts_with(prefix)).map(|v| format!("{command} {v}")).collect()
    }

    pub fn busy_suggestions(&self, history: impl DoubleEndedIterator<Item = impl AsRef<str>>) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut result: Vec<_> = history
            .rev()
            .filter(|s| s.as_ref().len() <= 512 && !s.as_ref().contains('\n'))
            .take(32)
            .map(|s| s.as_ref().to_string())
            .collect();
        result.extend(self.commands.iter().cloned());
        for command in ["/mode ", "/view ", "/agents ", "/theme ", "/font ", "/suggestions "] {
            result.extend(self.choices(command));
        }
        result
    }
}

impl Helper for PromptHelper {}
impl Highlighter for PromptHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(ui::dim(hint))
    }
}
impl Validator for PromptHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        Ok(if ctx.input().ends_with('\\') { ValidationResult::Incomplete } else { ValidationResult::Valid(None) })
    }
}
impl Hinter for PromptHelper {
    type Hint = String;
    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        if !self.enabled || line.is_empty() || pos != line.len() || line.contains('\n') {
            return None;
        }
        self.choices(line)
            .into_iter()
            .find(|s| s.len() > line.len())
            .map(|s| s[line.len()..].to_string())
            .or_else(|| self.history.hint(line, pos, ctx).filter(|s| s.len() <= 512 && !s.contains('\n')))
    }
}
impl Completer for PromptHelper {
    type Candidate = Pair;
    fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let options = self.choices(&line[..pos]);
        if !options.is_empty() {
            return Ok((0, options.into_iter().map(|s| Pair { display: s.clone(), replacement: s }).collect()));
        }
        let start = line[..pos].rfind(char::is_whitespace).map_or(0, |i| i + 1);
        if line[start..pos].starts_with('@') {
            let (offset, files) = self.files.complete(&line[start + 1..pos], pos - start - 1, ctx)?;
            return Ok((start + 1 + offset, files));
        }
        self.files.complete(line, pos, ctx)
    }
}

pub struct PromptKeys;
impl ConditionalEventHandler for PromptKeys {
    fn handle(&self, event: &Event, _: RepeatCount, _: bool, ctx: &EventContext) -> Option<Cmd> {
        if *event == Event::from(rustyline::KeyEvent::ctrl('C')) {
            Some(if ctx.line().is_empty() { Cmd::Interrupt } else { Cmd::Kill(Movement::WholeBuffer) })
        } else {
            Some(if !ctx.line().starts_with('/') && ctx.has_hint() && ctx.pos() == ctx.line().len() {
                Cmd::CompleteHint
            } else {
                Cmd::Complete
            })
        }
    }
}

pub const MAX_DRAFT: usize = 64 * 1024;

#[derive(Default)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
    pub submitted: bool,
    pub quit: bool,
    pub active: bool,
    pub truncated: bool,
    pub suggestions: Vec<String>,
    pending: Vec<u8>,
    paste: bool,
    escape_at: Option<Instant>,
    undo: Option<(String, usize)>,
    killed: String,
    paste_cr: bool,
}

impl Draft {
    fn previous(&self) -> usize {
        self.text[..self.cursor].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..].graphemes(true).next().map_or(self.cursor, |s| self.cursor + s.len())
    }
    fn save_undo(&mut self) {
        self.undo = Some((self.text.clone(), self.cursor));
    }
    fn insert(&mut self, text: &str) {
        if self.text.len() + text.len() > MAX_DRAFT {
            self.truncated = true;
            return;
        }
        self.save_undo();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
    fn delete(&mut self, start: usize, end: usize) {
        if start == end {
            return;
        }
        self.save_undo();
        self.killed = self.text[start..end].to_string();
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.truncated = false;
    }
    fn hint(&self) -> Option<&str> {
        if self.cursor != self.text.len() || self.text.is_empty() || self.text.contains('\n') {
            return None;
        }
        self.suggestions.iter().find_map(|s| s.strip_prefix(&self.text).filter(|suffix| !suffix.is_empty()))
    }

    /// Incremental UTF-8/CSI decoding. A paste is data, never a submit or shortcut.
    /// Returns true only when the user explicitly stops or submits.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        self.pending.extend_from_slice(bytes);
        let mut stop = false;
        while !self.pending.is_empty() {
            if self.pending.starts_with(b"\x1b[200~") {
                self.pending.drain(..6);
                self.paste = true;
                self.paste_cr = false;
                self.escape_at = None;
                continue;
            }
            if self.pending.starts_with(b"\x1b[201~") {
                self.pending.drain(..6);
                self.paste = false;
                self.escape_at = None;
                continue;
            }
            if self.pending[0] == 27 {
                if self.pending.len() == 1 {
                    self.escape_at.get_or_insert_with(Instant::now);
                    break;
                }
                if self.pending[1] == b'[' || self.pending[1] == b'O' {
                    let end = self
                        .pending
                        .iter()
                        .enumerate()
                        .skip(2)
                        .find(|(_, b)| (0x40..=0x7e).contains(*b))
                        .map(|(i, _)| i);
                    let Some(end) = end else {
                        break;
                    };
                    let sequence = self.pending.drain(..=end).collect::<Vec<_>>();
                    self.escape_at = None;
                    if self.paste {
                        continue;
                    }
                    match sequence.as_slice() {
                        b"\x1b[D" | b"\x1bOD" => self.cursor = self.previous(),
                        b"\x1b[C" | b"\x1bOC" => {
                            if self.cursor == self.text.len() {
                                if let Some(h) = self.hint().map(str::to_string) {
                                    self.insert(&h);
                                }
                            } else {
                                self.cursor = self.next();
                            }
                        }
                        b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" => self.cursor = 0,
                        b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" => self.cursor = self.text.len(),
                        b"\x1b[3~" => self.delete(self.cursor, self.next()),
                        b"\x1b[13;2u" | b"\x1b[27;2;13~" => self.insert("\n"),
                        _ => {}
                    }
                    continue;
                }
                self.pending.remove(0);
                self.escape_at = None;
                if self.pending.first() == Some(&b'\r') {
                    self.pending.remove(0);
                    self.insert("\n");
                    continue;
                }
                let meta = self.pending.remove(0);
                match meta {
                    b'b' => {
                        while self.cursor > 0
                            && self.text[..self.cursor].chars().next_back().is_some_and(char::is_whitespace)
                        {
                            self.cursor = self.previous();
                        }
                        while self.cursor > 0
                            && self.text[..self.cursor].chars().next_back().is_some_and(|c| !c.is_whitespace())
                        {
                            self.cursor = self.previous();
                        }
                    }
                    b'f' => {
                        while self.cursor < self.text.len()
                            && self.text[self.cursor..].chars().next().is_some_and(|c| !c.is_whitespace())
                        {
                            self.cursor = self.next();
                        }
                        while self.cursor < self.text.len()
                            && self.text[self.cursor..].chars().next().is_some_and(char::is_whitespace)
                        {
                            self.cursor = self.next();
                        }
                    }
                    _ => {}
                }
                continue;
            }
            self.escape_at = None;
            let first = self.pending[0];
            if first < 32 || first == 127 {
                self.pending.remove(0);
                if self.paste {
                    match first {
                        b'\r' => {
                            self.insert("\n");
                            self.paste_cr = true;
                        }
                        b'\n' => {
                            if !self.paste_cr {
                                self.insert("\n");
                            }
                            self.paste_cr = false;
                        }
                        b'\t' => {
                            self.insert("  ");
                            self.paste_cr = false;
                        }
                        _ => {
                            self.paste_cr = false;
                        }
                    }
                    continue;
                }
                match first {
                    3 => {
                        stop = true;
                        break;
                    }
                    b'\r' => {
                        if self.text.ends_with('\\') {
                            self.delete(self.text.len() - 1, self.text.len());
                            self.insert("\n");
                        } else if !self.text.trim().is_empty() && !self.truncated {
                            self.submitted = true;
                            stop = true;
                            break;
                        }
                    }
                    b'\n' => self.insert("\n"),
                    b'\t' => {
                        if let Some(h) = self.hint().map(str::to_string) {
                            self.insert(&h);
                        }
                    }
                    1 => self.cursor = 0,
                    5 => self.cursor = self.text.len(),
                    2 => self.cursor = self.previous(),
                    6 => self.cursor = self.next(),
                    8 | 127 => self.delete(self.previous(), self.cursor),
                    4 if self.text.is_empty() => {
                        self.quit = true;
                        self.submitted = true;
                        stop = true;
                        break;
                    }
                    4 => self.delete(self.cursor, self.next()),
                    21 => self.delete(0, self.cursor),
                    11 => self.delete(self.cursor, self.text.len()),
                    23 => {
                        let mut start = self.cursor;
                        while start > 0 && self.text[..start].chars().next_back().is_some_and(char::is_whitespace) {
                            start = self.text[..start].grapheme_indices(true).next_back().unwrap().0;
                        }
                        while start > 0 && self.text[..start].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
                            start = self.text[..start].grapheme_indices(true).next_back().unwrap().0;
                        }
                        self.delete(start, self.cursor);
                    }
                    25 => {
                        let killed = self.killed.clone();
                        self.insert(&killed);
                    }
                    31 => {
                        if let Some((text, cursor)) = self.undo.take() {
                            let old = std::mem::replace(&mut self.text, text);
                            let old_cursor = std::mem::replace(&mut self.cursor, cursor);
                            self.undo = Some((old, old_cursor));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            // Insert contiguous UTF-8 in one operation, including large pastes.
            // Hold an incomplete scalar until the next read; discard only malformed bytes.
            self.paste_cr = false;
            let run = self.pending.iter().position(|b| *b < 32 || *b == 127).unwrap_or(self.pending.len());
            let (valid, invalid) = match std::str::from_utf8(&self.pending[..run]) {
                Ok(_) => (run, 0),
                Err(e) => (e.valid_up_to(), e.error_len().unwrap_or(0)),
            };
            if valid > 0 {
                let text = String::from_utf8(self.pending.drain(..valid).collect()).unwrap();
                self.insert(&text);
            }
            if invalid > 0 {
                self.pending.drain(..invalid);
            }
            if valid == 0 && invalid == 0 {
                break;
            }
        }
        if self.pending.len() > 32 {
            self.pending.clear();
            self.escape_at = None;
        }
        stop
    }

    pub fn escape_expired(&mut self) -> bool {
        if !self.paste && self.escape_at.is_some_and(|t| t.elapsed().as_millis() >= 80) {
            self.pending.clear();
            self.escape_at = None;
            true
        } else {
            false
        }
    }

    /// A one-row horizontal viewport keeps the caret visible even in narrow PTYs.
    pub fn preview(&self, cells: usize) -> (String, usize, String) {
        let before = self.text[..self.cursor].replace('\n', "↵");
        let after = self.text[self.cursor..].replace('\n', "↵");
        let budget = cells.saturating_sub(1);
        let mut left = String::new();
        for g in before.graphemes(true).rev() {
            if left.width() + g.width() > budget {
                break;
            }
            left.insert_str(0, g);
        }
        let caret = left.width();
        let mut text = left;
        for g in after.graphemes(true) {
            if text.width() + g.width() > budget {
                break;
            }
            text.push_str(g);
        }
        let mut hint = String::new();
        if self.cursor == self.text.len() {
            if let Some(suffix) = self.hint() {
                for g in suffix.graphemes(true) {
                    if text.width() + hint.width() + g.width() > budget {
                        break;
                    }
                    hint.push_str(g);
                }
            }
        }
        (text, caret, hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_and_unicode_editing_never_submit_accidentally() {
        let mut d = Draft::default();
        for b in "\x1b[200~hello\n👩‍💻é\x1b[201~".as_bytes() {
            assert!(!d.feed(&[*b]));
        }
        assert_eq!(d.text, "hello\n👩‍💻é");
        d.feed(b"\x7f");
        assert_eq!(d.text, "hello\n👩‍💻");
        d.feed(b"\x7f");
        assert_eq!(d.text, "hello\n");
        d.feed(b"\x1b[D");
        d.feed(b"!");
        assert_eq!(d.text, "hello!\n");
        assert!(d.feed(b"\r"));
        assert!(d.submitted);
    }
    #[test]
    fn interrupt_preserves_draft_and_arrow_sequences_do_not_stop() {
        let mut d = Draft::default();
        d.feed(b"draft");
        assert!(!d.feed(b"\x1b"));
        assert!(!d.feed(b"[D"));
        assert!(d.feed(b"\x03"));
        assert_eq!(d.text, "draft");
        assert!(!d.submitted);
    }
    #[test]
    fn eof_quits_only_an_empty_busy_editor() {
        let mut d = Draft::default();
        d.feed(b"draft\x01\x04");
        assert_eq!(d.text, "raft");
        assert!(!d.quit && !d.submitted);
        d.feed(b"\x05\x15");
        assert!(d.text.is_empty());
        assert!(d.feed(b"\x04"));
        assert!(d.quit && d.submitted);
        // An EOF control byte inside paste is data, never a quit request.
        let mut pasted = Draft::default();
        assert!(!pasted.feed(b"\x1b[200~\x04\x1b[201~"));
        assert!(!pasted.quit && !pasted.submitted);
    }

    #[test]
    fn suggestions_only_append_at_the_end() {
        let mut d = Draft { suggestions: vec!["/mode careful".into()], ..Default::default() };
        d.feed(b"/mode c\t");
        assert_eq!(d.text, "/mode careful");
        d.feed(b"\x01\t");
        assert_eq!(d.text, "/mode careful");
        let h = PromptHelper::new(Path::new("/nonexistent"));
        assert_eq!(h.choices("/mode c"), ["/mode careful"]);
    }
    #[test]
    fn paste_crlf_controls_and_large_chunks() {
        let mut d = Draft::default();
        assert!(!d.feed(b"\x1b[200~a\r\nb\x03\x1b[201~"));
        assert_eq!(d.text, "a\nb");
        assert!(!d.submitted);
        let mut d = Draft::default();
        let before = Instant::now();
        for chunk in "x".repeat(MAX_DRAFT).as_bytes().chunks(4096) {
            d.feed(chunk);
        }
        assert_eq!(d.text.len(), MAX_DRAFT);
        assert!(before.elapsed().as_secs_f32() < 1.0);
        let mut d = Draft::default();
        d.feed(&[0xff, b'a']);
        assert_eq!(d.text, "a");
    }
    #[test]
    fn viewport_and_buffer_are_bounded() {
        let mut d = Draft::default();
        d.insert(&"界".repeat(MAX_DRAFT / 3));
        d.insert("too much");
        assert!(d.text.len() <= MAX_DRAFT);
        assert!(d.truncated);
        assert!(!d.feed(b"\r"), "a truncated paste cannot be submitted silently");
        d.feed(b"\x7f");
        assert!(!d.truncated);
        for width in [1, 12, 24, 40, 80] {
            let (s, caret, hint) = d.preview(width);
            assert!(s.width() + hint.width() < width);
            assert!(caret < width);
        }
    }
}
