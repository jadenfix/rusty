//! A small VT100/ANSI screen: enough of xterm to replay what rusty writes
//! (cursor movement, erase, SGR colour and weight, wide characters) and look
//! at the result cell by cell. No scroll regions, alternate screen or modes.

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
}

/// The right half of a wide character.
pub const WIDE_TAIL: char = '\0';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub pen: Pen,
}

const BLANK: Cell = Cell { ch: ' ', pen: Pen { fg: Color::Default, bg: Color::Default, bold: false, dim: false } };

/// Something the caller may want to look at the screen after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A printable character was placed on the screen.
    Printed(char),
    /// `ESC[0m` or `ESC[m`.
    Reset,
}

pub struct Screen {
    rows: usize,
    cols: usize,
    grid: Vec<Vec<Cell>>,
    row: usize,
    col: usize,
    /// The last column was written; the next character wraps first.
    wrap: bool,
    pen: Pen,
    saved: (usize, usize),
    /// Lines pushed off the top into scrollback.
    pub scrolled: usize,
}

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Screen {
        Screen {
            rows,
            cols,
            grid: vec![vec![BLANK; cols]; rows],
            row: 0,
            col: 0,
            wrap: false,
            pen: Pen::default(),
            saved: (0, 0),
            scrolled: 0,
        }
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn cell(&self, row: usize, col: usize) -> Cell {
        self.grid[row][col]
    }

    /// One row as text; the right half of a wide character adds nothing.
    pub fn line(&self, row: usize) -> String {
        self.grid[row].iter().map(|c| c.ch).filter(|&c| c != WIDE_TAIL).collect()
    }

    pub fn lines(&self) -> Vec<String> {
        (0..self.rows).map(|r| self.line(r)).collect()
    }

    /// Replays `text`, calling `on` after each event. A sequence cut off at
    /// the end of `text` is dropped.
    pub fn feed(&mut self, text: &str, mut on: impl FnMut(&Screen, Event)) {
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                if self.put(ch) {
                    on(self, Event::Printed(ch));
                }
                continue;
            }
            match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut fin = None;
                    for c in chars.by_ref() {
                        match c {
                            '0'..='?' | ' '..='/' => params.push(c),
                            '@'..='~' => {
                                fin = Some(c);
                                break;
                            }
                            _ => break, // malformed; drop it
                        }
                    }
                    if let Some(fin) = fin {
                        if self.csi(&params, fin) {
                            on(self, Event::Reset);
                        }
                    }
                }
                // OSC: skip to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                Some('7') => self.saved = (self.row, self.col),
                Some('8') => (self.row, self.col, self.wrap) = (self.saved.0, self.saved.1, false),
                // Charset designations and the like: ESC, intermediates, final.
                Some(' '..='/') => while chars.next().is_some_and(|c| (' '..='/').contains(&c)) {},
                _ => {}
            }
        }
    }

    /// Returns whether a character was placed.
    fn put(&mut self, ch: char) -> bool {
        match ch {
            '\r' => (self.col, self.wrap) = (0, false),
            '\n' | '\x0b' | '\x0c' => {
                self.wrap = false;
                self.down();
            }
            '\x08' => (self.col, self.wrap) = (self.col.saturating_sub(1), false),
            '\t' => (self.col, self.wrap) = (((self.col / 8 + 1) * 8).min(self.cols - 1), false),
            _ => {}
        }
        let width = match ch.width() {
            Some(w) if w > 0 && !ch.is_control() => w,
            _ => return false,
        };
        if self.wrap || self.col + width > self.cols {
            self.col = 0;
            self.wrap = false;
            self.down();
        }
        let width = width.min(self.cols);
        self.grid[self.row][self.col] = Cell { ch, pen: self.pen };
        if width == 2 {
            self.grid[self.row][self.col + 1] = Cell { ch: WIDE_TAIL, pen: self.pen };
        }
        if self.col + width >= self.cols {
            self.col = self.cols - 1;
            self.wrap = true;
        } else {
            self.col += width;
        }
        true
    }

    fn down(&mut self) {
        if self.row + 1 == self.rows {
            self.grid.remove(0);
            self.grid.push(vec![BLANK; self.cols]);
            self.scrolled += 1;
        } else {
            self.row += 1;
        }
    }

    fn erase(&mut self, row: usize, from: usize, to: usize) {
        for cell in &mut self.grid[row][from.min(self.cols)..to.min(self.cols)] {
            *cell = BLANK;
        }
    }

    /// Applies one control sequence; returns true for an SGR reset.
    fn csi(&mut self, params: &str, fin: char) -> bool {
        if params.starts_with(['?', '>', '<', '=']) || params.contains(|c: char| (' '..='/').contains(&c)) {
            return false; // private modes (bracketed paste, cursor shape): no effect on cells
        }
        let nums: Vec<usize> = params.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect();
        let n = nums[0];
        let n1 = n.max(1);
        let (last_row, last_col) = (self.rows - 1, self.cols - 1);
        if fin != 'm' {
            self.wrap = false;
        }
        match fin {
            'A' => self.row = self.row.saturating_sub(n1),
            'B' | 'e' => self.row = (self.row + n1).min(last_row),
            'C' | 'a' => self.col = (self.col + n1).min(last_col),
            'D' => self.col = self.col.saturating_sub(n1),
            'E' => (self.row, self.col) = ((self.row + n1).min(last_row), 0),
            'F' => (self.row, self.col) = (self.row.saturating_sub(n1), 0),
            'G' | '`' => self.col = (n1 - 1).min(last_col),
            'd' => self.row = (n1 - 1).min(last_row),
            'H' | 'f' => {
                self.row = (n1 - 1).min(last_row);
                self.col = (nums.get(1).copied().unwrap_or(0).max(1) - 1).min(last_col);
            }
            'K' => match n {
                0 => self.erase(self.row, self.col, self.cols),
                1 => self.erase(self.row, 0, self.col + 1),
                2 => self.erase(self.row, 0, self.cols),
                _ => {}
            },
            'J' => match n {
                0 => {
                    self.erase(self.row, self.col, self.cols);
                    (self.row + 1..self.rows).for_each(|r| self.erase(r, 0, self.cols));
                }
                1 => {
                    (0..self.row).for_each(|r| self.erase(r, 0, self.cols));
                    self.erase(self.row, 0, self.col + 1);
                }
                2 | 3 => (0..self.rows).for_each(|r| self.erase(r, 0, self.cols)),
                _ => {}
            },
            's' => self.saved = (self.row, self.col),
            'u' => (self.row, self.col) = self.saved,
            'm' => {
                self.sgr(&nums);
                return n == 0;
            }
            _ => {}
        }
        false
    }

    fn sgr(&mut self, nums: &[usize]) {
        let mut it = nums.iter().copied();
        while let Some(n) = it.next() {
            match n {
                0 => self.pen = Pen::default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                22 => (self.pen.bold, self.pen.dim) = (false, false),
                30..=37 => self.pen.fg = Color::Indexed((n - 30) as u8),
                90..=97 => self.pen.fg = Color::Indexed((n - 90 + 8) as u8),
                40..=47 => self.pen.bg = Color::Indexed((n - 40) as u8),
                100..=107 => self.pen.bg = Color::Indexed((n - 100 + 8) as u8),
                39 => self.pen.fg = Color::Default,
                49 => self.pen.bg = Color::Default,
                38 | 48 => {
                    let color = match it.next() {
                        Some(5) => Color::Indexed(it.next().unwrap_or(0) as u8),
                        Some(2) => {
                            let mut c = || it.next().unwrap_or(0) as u8;
                            Color::Rgb(c(), c(), c())
                        }
                        _ => continue,
                    };
                    if n == 38 {
                        self.pen.fg = color;
                    } else {
                        self.pen.bg = color;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: usize, cols: usize, text: &str) -> Screen {
        let mut s = Screen::new(rows, cols);
        s.feed(text, |_, _| {});
        s
    }

    #[test]
    fn moves_erases_and_wraps() {
        let s = screen(3, 5, "abcdefg\r\n\x1b[1A\x1b[2C\x1b[KX\x1b[3;2HY");
        assert_eq!(s.lines(), ["abcde", "fgX  ", " Y   "]);
        let s = screen(2, 4, "a\nb\nc\x1b[2J");
        assert_eq!((s.scrolled, s.lines()), (1, vec!["    ".to_string(), "    ".to_string()]));
    }

    #[test]
    fn colours_and_weight() {
        let s = screen(1, 6, "\x1b[1;38;5;102ma\x1b[0mb\x1b[38;2;1;2;3mc\x1b[22;39md");
        assert_eq!(s.cell(0, 0).pen, Pen { fg: Color::Indexed(102), bold: true, ..Pen::default() });
        assert_eq!(s.cell(0, 1).pen, Pen::default());
        assert_eq!(s.cell(0, 2).pen.fg, Color::Rgb(1, 2, 3));
        assert_eq!(s.cell(0, 3).pen, Pen::default());
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let s = screen(2, 5, "文件🦀x");
        assert_eq!(s.lines(), ["文件 ", "🦀x  "]);
        assert_eq!(s.cell(0, 1).ch, WIDE_TAIL);
        assert_eq!(s.cell(0, 4).ch, ' ');
    }

    #[test]
    fn reports_resets_and_skips_private_and_osc() {
        let mut events = Vec::new();
        let mut s = Screen::new(1, 8);
        s.feed("\x1b[?2004h\x1b]0;title\x07a\x1b[1mb\x1b[m", |_, e| events.push(e));
        assert_eq!(events, [Event::Printed('a'), Event::Printed('b'), Event::Reset]);
        assert_eq!(s.line(0), "ab      ");
    }
}
