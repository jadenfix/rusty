//! A primary-screen activity strip with a rotating Braille trefoil.
//! Logs enter ordinary scrollback; geometry and labels keep their own cells.
use std::io::Write;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use crate::execution::ExecutionMode;
use crate::ui;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Activity {
    #[default]
    Thinking,
    Working,
    Verifying,
    Responding,
    Subagent,
    Swarm,
    Reviewing,
}

impl Activity {
    fn label(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::Working => "working",
            Self::Verifying => "verifying",
            Self::Responding => "responding",
            Self::Subagent => "subagent",
            Self::Swarm => "swarm",
            Self::Reviewing => "checker",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pulse {
    Tool,
    Success,
    Error,
}

#[derive(Default)]
pub struct Footer {
    rows: usize,
    activity: Activity,
    pulse: Option<(Pulse, Instant)>,
    mode: ExecutionMode,
    workers: Vec<Option<bool>>,
    calls: usize,
}

impl Footer {
    pub fn mode(&mut self, mode: ExecutionMode) {
        self.mode = mode;
    }

    pub fn workers_start(&mut self, total: usize, review: bool) {
        self.workers = vec![None; total];
        self.calls = 0;
        self.activity = if review {
            Activity::Reviewing
        } else if total == 1 {
            Activity::Subagent
        } else {
            Activity::Swarm
        };
        self.pulse(Pulse::Tool);
    }

    pub fn worker_done(&mut self, index: usize, failed: bool) {
        if let Some(worker) = self.workers.get_mut(index) {
            *worker = Some(!failed);
        }
        self.pulse(if failed { Pulse::Error } else { Pulse::Success });
    }

    pub fn worker_calls(&mut self, calls: usize) {
        self.calls = calls;
    }

    pub fn workers_end(&mut self) {
        self.workers.clear();
        self.activity = Activity::Thinking;
    }

    pub fn activity(&mut self, activity: Activity) {
        self.activity = activity;
    }

    pub fn pulse(&mut self, pulse: Pulse) {
        self.pulse = Some((pulse, Instant::now()));
    }

    /// The cursor lives on the final strip row, never one row beyond it.
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
        let _ = write!(out, "\x1b[?25h");
        self.rows = 0;
    }

    pub fn draw(&mut self, out: &mut impl Write, frame: usize, verb: &str, status: &str, secs: u64) {
        self.clear(out);
        let _ = write!(out, "\x1b[?25l");
        let motion = ui::animate();
        let pulse = self.pulse.filter(|(_, at)| motion && at.elapsed().as_millis() < 900);
        let strength = pulse.map_or(0.0, |(_, at)| 1.0 - at.elapsed().as_secs_f32() / 0.9);
        let effect = pulse.map(|(p, _)| p);
        let returned = self.workers.iter().filter(|w| w.is_some()).count();
        let failed = self.workers.iter().filter(|w| **w == Some(false)).count();
        let progress = if self.workers.is_empty() {
            String::new()
        } else {
            format!(
                "{returned}/{} returned · {} running · {failed} failed · {} calls",
                self.workers.len(),
                self.workers.len() - returned,
                self.calls
            )
        };
        let mut nodes = self
            .workers
            .iter()
            .take(8)
            .enumerate()
            .map(|(i, state)| {
                format!(
                    "{}{}",
                    i + 1,
                    match state {
                        None => '●',
                        Some(true) => '✓',
                        Some(false) => '×',
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        if self.workers.len() > 8 {
            nodes.push_str(&format!(" +{}", self.workers.len() - 8));
        }
        if !nodes.is_empty() {
            nodes.push_str(" · read-only");
        }
        let lines = panel(
            ui::width(),
            ui::height(),
            if motion { frame } else { 0 },
            self.activity,
            verb,
            if self.workers.is_empty() { status } else { &progress },
            secs,
            strength,
            effect,
            self.mode,
            &nodes,
        );
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                let _ = write!(out, "\r\n");
            }
            let _ = write!(out, "{}", styled(line, i, lines.len() == 8, effect, strength, self.mode, self.activity));
        }
        self.rows = lines.len();
        let cells = ui::width().saturating_sub(4);
        if ui::height() >= 2 {
            if let Some((text, caret, hint)) = crate::signal::composer(cells) {
                let _ = write!(out, "\r\n{} {}{}", ui::primary("›"), text, ui::dim(&hint));
                let _ = write!(out, "\r\x1b[{}C\x1b[?25h", caret + 2);
                self.rows += 1;
            }
        }
    }
}

/// Paint contiguous runs. Pulses affect the sculpture, never the borders or text.
fn styled(
    line: &str,
    row: usize,
    expanded: bool,
    pulse: Option<Pulse>,
    strength: f32,
    mode: ExecutionMode,
    activity: Activity,
) -> String {
    let brand = line.find("rusty").map(|i| i..i + 5);
    let paint = |kind, text: &str| match kind {
        1 => text.to_string(),
        2 => {
            let ink = match pulse {
                Some(Pulse::Error) => ui::accent(text),
                Some(Pulse::Success) => ui::info(text),
                _ if activity == Activity::Reviewing || mode == ExecutionMode::Careful => ui::warn(text),
                _ if matches!(activity, Activity::Subagent | Activity::Swarm) => ui::info(text),
                _ => ui::primary(text),
            };
            if strength > 0.45 {
                ui::bold(&ink)
            } else {
                ink
            }
        }
        3 => ui::bold(&ui::primary(text)),
        4 => ui::info(text),
        _ => ui::dim(text),
    };
    let mut result = String::new();
    let mut run = String::new();
    let mut last = 0;
    for (i, c) in line.char_indices() {
        let kind = if ('\u{2801}'..='\u{28ff}').contains(&c) || c == '◇' {
            2
        } else if brand.as_ref().is_some_and(|r| r.contains(&i)) {
            3
        } else if "─│".contains(c) {
            4
        } else if (expanded && (row == 2 || row == 3) || !expanded && row == 1) && c != '│' {
            1
        } else {
            0
        };
        if kind != last && !run.is_empty() {
            result.push_str(&paint(last, &run));
            run.clear();
        }
        run.push(c);
        last = kind;
    }
    result.push_str(&paint(last, &run));
    result
}

/// Clip by terminal cells, remove control characters, leave the wrap column free.
fn fit(s: &str, cells: usize) -> String {
    let mut text = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        text.push(c);
        if text.width() > cells {
            text.pop();
            if cells > 0 {
                while text.width() + 1 > cells {
                    text.pop();
                }
                text.push('…');
            }
            break;
        }
    }
    text
}

const ART_W: usize = 16;
const ART_H: usize = 6;

/// Project a tubular trefoil onto a fixed 32×24-dot Braille canvas.
/// A tiny stack depth buffer gives crossings real occlusion; no assets or crates.
fn knot(frame: usize, pulse: f32, effect: Option<Pulse>, mode: ExecutionMode, activity: Activity) -> [String; ART_H] {
    const W: usize = ART_W * 2;
    const H: usize = ART_H * 4;
    let mut depth = [f32::NEG_INFINITY; W * H];
    let speed = if activity == Activity::Reviewing {
        0.022
    } else if matches!(activity, Activity::Subagent | Activity::Swarm) {
        0.055
    } else if mode == ExecutionMode::Careful {
        0.028
    } else {
        0.04
    };
    let angle = (frame as f32 * speed) % std::f32::consts::TAU;
    let (spin_s, spin_c) = angle.sin_cos();
    // Rotate around the sculpture's face while gently changing its depth angle.
    // A full edge-on yaw would collapse this small canvas into an unreadable line.
    let (sa, ca) = (0.3 + 0.2 * (frame as f32 * 0.028).sin()).sin_cos();
    let (st, ct) = 0.65_f32.sin_cos();
    // Slow breathing; a short decaying ripple on actual lifecycle events.
    let ripple = match effect {
        Some(Pulse::Error) => (pulse * 18.0).sin().abs(),
        _ => (pulse * std::f32::consts::PI).sin(),
    };
    let scale = 3.0 * (1.0 + 0.015 * (angle * 2.0).sin() + 0.09 * ripple * pulse);
    for i in 0..192 {
        let t = i as f32 * std::f32::consts::TAU / 192.0;
        let (s2, c2) = (2.0 * t).sin_cos();
        let (s3, c3) = (3.0 * t).sin_cos();
        let radius = 2.0 + c3;
        let center = [radius * c2, radius * s2, s3];
        let tangent = [-3.0 * s3 * c2 - 2.0 * radius * s2, -3.0 * s3 * s2 + 2.0 * radius * c2, 3.0 * c3];
        let n = tangent[0].hypot(tangent[1]);
        let normal = [-tangent[1] / n, tangent[0] / n, 0.0];
        let length = n.hypot(tangent[2]);
        let binormal = [
            -tangent[2] * normal[1] / length,
            tangent[2] * normal[0] / length,
            (tangent[0] * normal[1] - tangent[1] * normal[0]) / length,
        ];
        for j in 0..16 {
            let (s, c) = (j as f32 * std::f32::consts::TAU / 16.0).sin_cos();
            let p = std::array::from_fn::<_, 3, _>(|k| center[k] + 0.34 * (normal[k] * c + binormal[k] * s));
            let rotated_x = p[0] * spin_c - p[1] * spin_s;
            let rotated_y = p[0] * spin_s + p[1] * spin_c;
            let x = rotated_x * ca + p[2] * sa;
            let z = -rotated_x * sa + p[2] * ca;
            let y = rotated_y * ct - z * st;
            let z = rotated_y * st + z * ct;
            let perspective = 9.0 / (9.0 - z);
            let px = (W as f32 / 2.0 + x * scale * perspective).round() as isize;
            let py = (H as f32 / 2.0 - y * scale * perspective).round() as isize;
            if px >= 0 && px < W as isize && py >= 0 && py < H as isize {
                let at = py as usize * W + px as usize;
                depth[at] = depth[at].max(z);
            }
        }
    }
    if mode == ExecutionMode::Careful {
        // An angular protective silhouette; it stays fixed as the knot rotates.
        let shield =
            [(16.0_f32, 1.0_f32), (28.0, 5.0), (26.0, 16.0), (16.0, 23.0), (6.0, 16.0), (4.0, 5.0), (16.0, 1.0)];
        for edge in shield.windows(2) {
            for i in (0..24).step_by(2) {
                let t = i as f32 / 24.0;
                let x = (edge[0].0 + (edge[1].0 - edge[0].0) * t).round() as usize;
                let y = (edge[0].1 + (edge[1].1 - edge[0].1) * t).round() as usize;
                depth[y * W + x] = 9.0;
            }
        }
    }
    if matches!(activity, Activity::Subagent | Activity::Swarm | Activity::Reviewing) {
        let count = if activity == Activity::Swarm { 3 } else { 1 };
        for node in 0..count {
            let phase = angle * 1.4 + node as f32 * std::f32::consts::TAU / count as f32;
            for tail in 0..3 {
                let phase = phase - tail as f32 * 0.1;
                let x = (16.0 + phase.cos() * 13.0).round() as usize;
                let y = (11.5 + phase.sin() * 10.0).round() as usize;
                depth[y * W + x] = 9.0;
            }
        }
    }
    std::array::from_fn(|row| {
        (0..ART_W)
            .map(|col| {
                let mut bits = 0u32;
                for (dy, pair) in [[0, 3], [1, 4], [2, 5], [6, 7]].iter().enumerate() {
                    for (dx, bit) in pair.iter().enumerate() {
                        let x = col * 2 + dx;
                        let y = row * 4 + dy;
                        let z = depth[y * W + x];
                        // Sparse back-facing dots make the rotating surface readable.
                        if z.is_finite() && (z > 0.0 || (x + y) % 3 != 0) {
                            bits |= 1 << bit;
                        }
                    }
                }
                if bits == 0 {
                    ' '
                } else {
                    char::from_u32(0x2800 + bits).unwrap()
                }
            })
            .collect()
    })
}

#[allow(clippy::too_many_arguments)]
fn panel(
    width: usize,
    height: usize,
    frame: usize,
    activity: Activity,
    verb: &str,
    status: &str,
    secs: u64,
    pulse: f32,
    effect: Option<Pulse>,
    mode: ExecutionMode,
    workers: &str,
) -> Vec<String> {
    let width = width.saturating_sub(1).min(88);
    if width < 30 || height < 8 {
        return vec![fit(&format!("◇ {}/{} · {status}", mode.name(), activity.label()), width)];
    }
    let inset = if width >= 39 { "  " } else { "" };
    let width = width - inset.len();
    let rule = format!("{inset}{}", "─".repeat(width));
    let mut lines = vec![rule.clone()];
    if width >= 57 && height >= 20 {
        let title = format!("rusty / {}", activity.label());
        let time = format!(" {} · {secs}s", mode.name());
        let room = width - ART_W - 3;
        let title = fit(&title, room.saturating_sub(time.width()));
        let heading = format!("{title}{}{time}", " ".repeat(room.saturating_sub(title.width() + time.width())));
        let detail = if status.is_empty() { verb.to_string() } else { status.to_string() };
        let controls = if crate::signal::draft_limited() {
            "draft limit 64KiB · edit before sending"
        } else if crate::signal::composer(1).is_some() {
            "Esc/Ctrl-C stop · Enter steer · Tab hint"
        } else {
            "ctrl-c to stop"
        };
        let labels = ["", heading.as_str(), detail.as_str(), workers, controls, ""];
        for (face, label) in knot(frame, pulse, effect, mode, activity).iter().zip(labels) {
            let body = fit(&format!("{face} │ {label}"), width);
            lines.push(format!("{inset}{body}{}", " ".repeat(width.saturating_sub(body.width()))));
        }
    } else {
        let detail = if status.is_empty() { verb.to_string() } else { status.to_string() };
        let body = fit(&format!("rusty / {} / {} · {detail} · {secs}s", mode.name(), activity.label()), width);
        lines.push(format!("{inset}{body}{}", " ".repeat(width.saturating_sub(body.width()))));
    }
    lines.push(rule);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_states_and_counts_are_independent_of_animation() {
        let mut footer = Footer::default();
        footer.mode(ExecutionMode::Careful);
        footer.workers_start(3, false);
        assert_eq!(footer.activity, Activity::Swarm);
        footer.worker_done(1, true);
        footer.worker_done(0, false);
        assert_eq!(footer.workers, [Some(true), Some(false), None]);
        footer.workers_end();
        assert!(footer.workers.is_empty());
        assert_eq!(footer.mode, ExecutionMode::Careful);
        footer.workers_start(1, true);
        assert_eq!(footer.activity, Activity::Reviewing);
        footer.worker_calls(7);
        assert_eq!(footer.calls, 7);
        assert_ne!(
            knot(0, 0.0, None, ExecutionMode::Standard, Activity::Thinking),
            knot(0, 0.0, None, ExecutionMode::Careful, Activity::Reviewing)
        );
    }

    #[test]
    fn mode_and_delegation_layouts_keep_the_same_geometry() {
        for width in [24, 40, 80, 120] {
            for activity in [Activity::Subagent, Activity::Swarm, Activity::Reviewing] {
                let lines = panel(
                    width,
                    24,
                    20,
                    activity,
                    "Thinking",
                    "2/3 returned · 1 running · 1 failed · 7 calls",
                    13,
                    0.0,
                    None,
                    ExecutionMode::Careful,
                    "1✓ 2× 3● · read-only",
                );
                assert!(lines.iter().all(|line| line.width() < width));
                assert!(lines.iter().any(|line| line.contains("careful")));
                assert!(lines.iter().any(|line| line.contains(activity.label())));
            }
        }
    }

    #[test]
    fn frames_do_not_wrap_at_any_terminal_size() {
        for width in [1, 12, 24, 31, 40, 80, 120, 300] {
            for height in [3, 8, 18, 20, 24] {
                for frame in 0..12 {
                    let lines = panel(
                        width,
                        height,
                        frame,
                        Activity::Working,
                        "Thinking",
                        "文件 🦀 very long\r\n\x1b",
                        12345,
                        0.7,
                        Some(Pulse::Tool),
                        ExecutionMode::Careful,
                        "1● 2✓ 3× · read-only",
                    );
                    assert!(lines.iter().all(|line| line.width() < width));
                    assert!(lines.iter().all(|line| !line.chars().any(char::is_control)));
                    assert_eq!(
                        lines.len(),
                        if width < 31 || height < 8 {
                            1
                        } else if width >= 60 && height >= 20 {
                            8
                        } else {
                            3
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn rotation_and_pulses_only_change_the_art_cells() {
        let still =
            panel(80, 24, 0, Activity::Verifying, "Checking", "routes", 4, 0.0, None, ExecutionMode::Standard, "");
        for (frame, pulse, effect) in [(17, 0.0, None), (0, 0.7, Some(Pulse::Tool)), (0, 0.6, Some(Pulse::Error))] {
            let animated = panel(
                80,
                24,
                frame,
                Activity::Verifying,
                "Checking",
                "routes",
                4,
                pulse,
                effect,
                ExecutionMode::Standard,
                "",
            );
            assert_ne!(still, animated);
            assert_eq!(animated.len(), 8);
            assert!(animated.iter().all(|line| line.width() == 79));
            for row in 0..8 {
                if row == 0 || row == 7 {
                    assert_eq!(still[row], animated[row]);
                } else {
                    assert_eq!(
                        still[row].chars().skip(18).collect::<String>(),
                        animated[row].chars().skip(18).collect::<String>()
                    );
                }
            }
        }
    }
}
