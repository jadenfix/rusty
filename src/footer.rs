//! A small primary-screen panel. Completed lines enter ordinary scrollback;
//! only the geometric rail moves. No alternate screen or animation thread.
use std::io::Write;
use unicode_width::UnicodeWidthStr;

use crate::ui;

#[derive(Default)]
pub struct Footer {
    rows: usize,
}

impl Footer {
    /// The cursor lives on the final panel row, never one row beyond it.
    pub fn clear(&mut self, out: &mut impl Write) {
        if self.rows == 0 {
            return;
        }
        let _ = write!(out, "\r");
        if self.rows > 1 {
            let _ = write!(out, "\x1b[{}A", self.rows - 1);
        }
        for row in 0..self.rows {
            let _ = write!(out, "\x1b[2K");
            if row + 1 < self.rows {
                let _ = write!(out, "\x1b[1B\r");
            }
        }
        if self.rows > 1 {
            let _ = write!(out, "\x1b[{}A", self.rows - 1);
        }
        self.rows = 0;
    }

    pub fn draw(&mut self, out: &mut impl Write, frame: usize, verb: &str, status: &str, secs: u64) {
        self.clear(out);
        let lines = panel(ui::width(), ui::height(), frame, verb, status, secs);
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                let _ = write!(out, "\r\n");
            }
            let _ = write!(out, "{}", if i == 1 { ui::primary(line) } else { ui::dim(line) });
        }
        self.rows = lines.len();
    }
}

/// Clips by terminal cells, removes control characters, and leaves the final
/// terminal column unused so a frame cannot trigger automatic wrapping.
fn fit(s: &str, cells: usize) -> String {
    let mut text = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        text.push(c);
        if text.width() > cells {
            text.pop();
            break;
        }
    }
    text
}

fn boxed(left: &str, body: &str, right: &str, width: usize, fill: &str) -> String {
    let inside = width.saturating_sub(2);
    let body = fit(body, inside);
    format!("{left}{body}{}{right}", fill.repeat(inside.saturating_sub(body.width())))
}

fn panel(width: usize, height: usize, frame: usize, verb: &str, status: &str, secs: u64) -> Vec<String> {
    let width = width.saturating_sub(1);
    if width < 30 || height < 8 {
        return vec![fit(&format!("◇ rusty · {secs}s · {verb} {status}"), width)];
    }
    // Three falling diamonds, staggered through three fixed rows. Motion
    // stays at the left; status and time retain the same anchors each frame.
    let phase = frame / 2 % 6;
    let rail = |row: usize| -> String {
        (0..3)
            .map(|col| {
                if (phase + col * 2) % 6 / 2 == row {
                    if phase.is_multiple_of(2) {
                        '◇'
                    } else {
                        '◆'
                    }
                } else {
                    '·'
                }
            })
            .collect()
    };
    let time = format!(" {secs:>4}s ─");
    let title = format!("─ {} rusty ", rail(0));
    let header = format!("{title}{}{time}", "─".repeat(width.saturating_sub(2 + title.width() + time.width())));
    let status = if status.is_empty() { verb.to_string() } else { format!("{verb} · {status}") };
    vec![
        boxed("╭", &header, "╮", width, "─"),
        boxed("│", &format!("  {}  {status}", rail(1)), "│", width, " "),
        boxed("╰", &format!("─ {} ─ ctrl-c to stop ", rail(2)), "╯", width, "─"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_do_not_wrap_at_any_terminal_size() {
        for width in [1, 12, 24, 31, 40, 80, 120, 300] {
            for height in [3, 24] {
                for frame in 0..12 {
                    let lines = panel(width, height, frame, "Thinking", "文件 🦀 very long status\r\n\x1b", 12345);
                    assert!(lines.iter().all(|line| line.width() < width));
                    assert!(lines.iter().all(|line| !line.chars().any(char::is_control)));
                    assert_eq!(lines.len(), if width < 31 || height < 8 { 1 } else { 3 });
                }
            }
        }
    }

    #[test]
    fn diamonds_fall_without_moving_the_panel() {
        let frames: Vec<_> = (0..12).map(|f| panel(80, 24, f, "Thinking", "reading", 4)).collect();
        assert!(frames.windows(2).any(|f| f[0] != f[1]));
        for lines in frames {
            assert_eq!(lines.len(), 3);
            assert!(lines.iter().all(|line| line.width() == 79));
            assert!(lines[0].ends_with("    4s ─╮"));
        }
    }
}
