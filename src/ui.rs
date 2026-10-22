//! Rendering. Pure function of `App` state: no I/O, no timing decisions.

use crate::app::{App, Overlay};
use crate::config::Theme;
use crate::timer::format_remaining;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use std::time::Instant;

const CARD_WIDTH: u16 = 52;

/// Terminal capabilities and colours, detected once at startup.
pub struct Look {
    pub unicode: bool,
    pub pal: Palette,
}

pub struct Palette {
    pub accent: Style,
    pub paused: Style,
    pub done: Style,
    pub error: Style,
    pub dim: Style,
    pub border: Style,
    pub bar_empty: Style,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    None,
    Basic,
    Indexed,
    True,
}

impl ColorDepth {
    pub fn detect() -> ColorDepth {
        let env = |k| std::env::var(k).unwrap_or_default();
        if !env("NO_COLOR").is_empty() {
            return ColorDepth::None;
        }
        let (term, colorterm) = (env("TERM"), env("COLORTERM"));
        if term == "dumb" {
            return ColorDepth::None;
        }
        if colorterm == "truecolor" || colorterm == "24bit" {
            return ColorDepth::True;
        }
        if term.contains("256")
            || ["kitty", "alacritty", "foot", "wezterm", "ghostty"]
                .iter()
                .any(|t| term.contains(t))
        {
            return ColorDepth::Indexed;
        }
        ColorDepth::Basic
    }
}

pub fn detect_unicode() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    // The Linux VT console lacks many glyphs even with a UTF-8 locale.
    if matches!(term.as_str(), "linux" | "dumb" | "vt100" | "vt220") {
        return false;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    locale.contains("utf-8") || locale.contains("utf8")
}

impl Palette {
    pub fn new(theme: Theme, depth: ColorDepth) -> Palette {
        let fg = |c: Color| Style::new().fg(c);
        if theme == Theme::Mono || depth == ColorDepth::None {
            let plain = Style::new();
            return Palette {
                accent: plain.add_modifier(Modifier::BOLD),
                paused: plain.add_modifier(Modifier::BOLD),
                done: plain.add_modifier(Modifier::BOLD),
                error: plain.add_modifier(Modifier::BOLD),
                dim: plain.add_modifier(Modifier::DIM),
                border: plain.add_modifier(Modifier::DIM),
                bar_empty: plain.add_modifier(Modifier::DIM),
            };
        }
        // Calm palette: sage-teal accent, amber for pause, green for done, neutral greys.
        let [accent, paused, done, error, dim, border, empty] = match depth {
            ColorDepth::True => [
                Color::Rgb(125, 207, 182),
                Color::Rgb(229, 192, 123),
                Color::Rgb(152, 195, 121),
                Color::Rgb(224, 108, 117),
                Color::Rgb(124, 132, 145),
                Color::Rgb(84, 92, 105),
                Color::Rgb(62, 68, 80),
            ],
            ColorDepth::Indexed => [79, 179, 107, 167, 245, 240, 238].map(Color::Indexed),
            _ => [
                Color::Cyan,
                Color::Yellow,
                Color::Green,
                Color::Red,
                Color::Gray,
                Color::DarkGray,
                Color::DarkGray,
            ],
        };
        Palette {
            accent: fg(accent),
            paused: fg(paused),
            done: fg(done),
            error: fg(error),
            dim: fg(dim),
            border: fg(border),
            bar_empty: fg(empty),
        }
    }
}

impl Look {
    fn border_set(&self) -> border::Set<'static> {
        if self.unicode {
            border::ROUNDED
        } else {
            border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        }
    }
}

// --- Big digits ------------------------------------------------------------------------

/// 3x5 bitmap font; each pixel is drawn two cells wide to keep a square-ish aspect.
const FONT: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b110, 0b010, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
const COLON: [u8; 5] = [0, 1, 0, 1, 0];
const BIG_ROWS: usize = 5;

pub fn big_text(text: &str, unicode: bool) -> Vec<String> {
    let on = if unicode { "\u{2588}\u{2588}" } else { "##" };
    let mut rows = vec![String::new(); BIG_ROWS];
    for (i, ch) in text.chars().enumerate() {
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 {
                row.push_str("  ");
            }
            match ch.to_digit(10) {
                Some(d) => {
                    for bit in (0..3).rev() {
                        row.push_str(if FONT[d as usize][r] >> bit & 1 == 1 {
                            on
                        } else {
                            "  "
                        });
                    }
                }
                None => row.push_str(if COLON[r] == 1 { on } else { "  " }),
            }
        }
    }
    rows
}

fn big_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c == ':' { 2 } else { 6 })
        .sum::<usize>()
        + 2 * text.chars().count().saturating_sub(1)
}

// --- Progress bar ------------------------------------------------------------------------

pub fn progress_bar(ratio: f64, width: usize, unicode: bool) -> (String, String) {
    let ratio = ratio.clamp(0.0, 1.0);
    if !unicode {
        let filled = (ratio * width as f64).round() as usize;
        return ("#".repeat(filled), "-".repeat(width - filled));
    }
    // Eighth-block resolution makes the bar advance smoothly.
    const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (ratio * width as f64 * 8.0).round() as usize;
    let (full, rem) = (eighths / 8, eighths % 8);
    let mut fill = "█".repeat(full);
    let mut used = full;
    if rem > 0 && used < width {
        fill.push(PARTIAL[rem]);
        used += 1;
    }
    (fill, "░".repeat(width - used))
}

// --- Layout --------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Variant {
    big: bool,
    keys: bool,
    pad: bool,
}

fn content_height(v: Variant) -> u16 {
    // header(2) + gap + timer + gap + bar + gap + status(2) [+ gap + keys(3)] [+ pad*2]
    let timer = if v.big { BIG_ROWS as u16 } else { 1 };
    2 + 1 + timer + 1 + 1 + 1 + 2 + if v.keys { 4 } else { 0 } + if v.pad { 2 } else { 0 }
}

fn pick_variant(area: Rect, timer_text: &str) -> Option<Variant> {
    let inner_w = area.width.min(CARD_WIDTH).saturating_sub(2) as usize;
    let big_fits = inner_w >= big_width(timer_text) + 4;
    let keys_fit = inner_w >= 36;
    [
        Variant {
            big: true,
            keys: true,
            pad: true,
        },
        Variant {
            big: true,
            keys: true,
            pad: false,
        },
        Variant {
            big: true,
            keys: false,
            pad: false,
        },
        Variant {
            big: false,
            keys: true,
            pad: false,
        },
        Variant {
            big: false,
            keys: false,
            pad: false,
        },
    ]
    .into_iter()
    .filter(|v| (!v.big || big_fits) && (!v.keys || keys_fit))
    .find(|v| content_height(*v) + 2 <= area.height)
}

/// A line with `left` flush-left and `right` flush-right inside `width` columns.
fn split_line<'a>(left: Vec<Span<'a>>, right: Vec<Span<'a>>, width: usize) -> Line<'a> {
    let lw: usize = left.iter().map(Span::width).sum();
    let rw: usize = right.iter().map(Span::width).sum();
    let gap = width.saturating_sub(lw + rw).max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

struct State {
    timer_style: Style,
    subtitle: &'static str,
    icon: &'static str,
    label: &'static str,
    style: Style,
}

fn state(app: &App, look: &Look) -> State {
    let u = look.unicode;
    let p = &look.pal;
    let (subtitle, icon, label, style) = if app.finished {
        (
            "SESSION COMPLETE",
            if u { "✓" } else { "*" },
            "Done. Take a break",
            p.done,
        )
    } else if app.timer.is_paused() {
        ("PAUSED", if u { "◌" } else { "o" }, "Paused", p.paused)
    } else {
        (
            "FOCUS SESSION",
            if u { "◉" } else { "@" },
            "Focusing",
            p.accent,
        )
    };
    State {
        timer_style: style.add_modifier(Modifier::BOLD),
        subtitle,
        icon,
        label,
        style,
    }
}

fn key<'a>(k: &'a str, label: &'a str, p: &Palette, col: usize) -> Vec<Span<'a>> {
    let used = k.len() + 3 + label.len();
    vec![
        Span::styled(format!("[{k}]"), p.accent),
        Span::raw(" "),
        Span::styled(label, p.dim),
        Span::raw(" ".repeat(col.saturating_sub(used))),
    ]
}

fn build_lines<'a>(
    app: &App,
    now: Instant,
    look: &Look,
    inner_w: usize,
    v: Variant,
) -> Vec<Line<'a>> {
    let p = &look.pal;
    let st = state(app, look);
    let remaining = format_remaining(app.timer.remaining(now));
    let body_w = inner_w.saturating_sub(4); // two columns of padding each side
    let mut lines: Vec<Line> = Vec::new();
    let blank = || Line::raw("");

    if v.pad {
        lines.push(blank());
    }
    lines.push(Line::styled("W O R K", p.accent.add_modifier(Modifier::BOLD)).centered());
    lines.push(Line::styled(st.subtitle, p.dim).centered());
    lines.push(blank());

    if v.big {
        for row in big_text(&remaining, look.unicode) {
            lines.push(Line::styled(row, st.timer_style).centered());
        }
    } else {
        lines.push(Line::styled(remaining, st.timer_style).centered());
    }
    lines.push(blank());

    // Progress bar with percentage.
    let pct = (app.timer.progress(now) * 100.0).floor() as u32;
    let bar_w = body_w.saturating_sub(6);
    let (fill, empty) = progress_bar(app.timer.progress(now), bar_w, look.unicode);
    lines.push(
        Line::from(vec![
            Span::styled(fill, st.style),
            Span::styled(empty, p.bar_empty),
            Span::styled(format!("{pct:>5}%"), p.dim),
        ])
        .centered(),
    );
    lines.push(blank());

    // Status rows.
    let total_min = app.timer.total().as_secs() / 60;
    let status = split_line(
        vec![Span::styled(format!("{} {}", st.icon, st.label), st.style)],
        vec![Span::styled(format!("{total_min} min session"), p.dim)],
        body_w,
    );
    let sound = if let Some(err) = &app.audio_error {
        let mut msg = format!("{} Audio unavailable", if look.unicode { "⚠" } else { "!" });
        if body_w > 40 {
            msg = format!("{msg}: {err}");
        }
        let max = body_w;
        if msg.chars().count() > max {
            msg = msg.chars().take(max.saturating_sub(1)).collect::<String>()
                + if look.unicode { "…" } else { "." };
        }
        Line::from(Span::styled(msg, p.error))
    } else {
        let (icon, style) = if app.muted || app.volume == 0 {
            (if look.unicode { "◇" } else { "-" }, p.dim)
        } else {
            (if look.unicode { "◈" } else { "~" }, p.accent)
        };
        let right = if app.muted {
            Span::styled("Muted", p.paused)
        } else {
            Span::styled(format!("Volume {}%", app.volume), p.dim)
        };
        split_line(
            vec![
                Span::styled(icon, style),
                Span::raw(" "),
                Span::raw(app.noise.label()),
            ],
            vec![right],
            body_w,
        )
    };
    let pad = |l: Line<'a>| {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(l.spans);
        Line::from(spans)
    };
    lines.push(pad(status));
    lines.push(pad(sound));

    if v.keys {
        lines.push(blank());
        let col = body_w / 2;
        let rows: [[(&str, &str); 2]; 3] = if app.finished {
            [
                [("R", "New session"), ("Q", "Quit")],
                [("N", "Noise"), ("+/-", "Volume")],
                [("?", "Help"), ("", "")],
            ]
        } else {
            [
                [
                    (
                        "Space",
                        if app.timer.is_paused() {
                            "Resume"
                        } else {
                            "Pause"
                        },
                    ),
                    ("+/-", "Volume"),
                ],
                [
                    ("N", "Noise"),
                    ("M", if app.muted { "Unmute" } else { "Mute" }),
                ],
                [("R", "Restart"), ("Q", "Quit")],
            ]
        };
        for row in rows {
            let mut spans = vec![Span::raw("  ")];
            for (k, label) in row {
                if !k.is_empty() {
                    spans.extend(key(k, label, p, col));
                }
            }
            lines.push(Line::from(spans));
        }
    }
    if v.pad {
        lines.push(blank());
    }
    lines
}

pub fn draw(f: &mut Frame, app: &App, now: Instant, look: &Look) {
    let area = f.area();
    let p = &look.pal;
    let remaining = format_remaining(app.timer.remaining(now));

    let Some(v) = pick_variant(area, &remaining) else {
        // Tiny terminal: just the time, centred, in the state colour.
        let st = state(app, look);
        let r = area.centered(
            Constraint::Length(area.width),
            Constraint::Length(1.min(area.height)),
        );
        f.render_widget(
            Paragraph::new(Line::styled(remaining, st.timer_style)).alignment(Alignment::Center),
            r,
        );
        return;
    };

    let w = area.width.min(CARD_WIDTH);
    let inner_w = w.saturating_sub(2) as usize;
    let lines = build_lines(app, now, look, inner_w, v);
    let card = area.centered(
        Constraint::Length(w),
        Constraint::Length(lines.len() as u16 + 2),
    );

    let mut block = Block::bordered()
        .border_set(look.border_set())
        .border_style(p.border);
    if v.keys || inner_w >= 20 {
        block = block.title_bottom(
            Line::from(vec![
                Span::styled(" ? ", p.accent),
                Span::styled("help ", p.dim),
            ])
            .right_aligned(),
        );
    }
    f.render_widget(Paragraph::new(lines).block(block), card);

    match app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help(f, area, look),
        Overlay::ConfirmRestart => draw_confirm(f, area, look),
    }
}

fn popup(f: &mut Frame, area: Rect, look: &Look, title: &str, lines: Vec<Line>) {
    let w = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 6;
    let h = lines.len() as u16 + 2;
    let r = area.centered(
        Constraint::Length(w.min(area.width)),
        Constraint::Length(h.min(area.height)),
    );
    let block = Block::bordered()
        .border_set(look.border_set())
        .border_style(look.pal.accent)
        .title(
            Line::styled(
                format!(" {title} "),
                look.pal.accent.add_modifier(Modifier::BOLD),
            )
            .centered(),
        );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Left),
        r,
    );
}

fn draw_help(f: &mut Frame, area: Rect, look: &Look) {
    let p = &look.pal;
    let row = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{k:<9}"), p.accent),
            Span::raw(d),
        ])
    };
    let lines = vec![
        Line::raw(""),
        row("Space", "Pause / resume"),
        row("+  -", "Volume up / down"),
        row("N", "Next noise: white, pink, brown"),
        row("M", "Mute / unmute"),
        row("R", "Restart session"),
        row("Q", "Quit"),
        row("?", "This help"),
        Line::raw(""),
        Line::styled("  Press any key to close", p.dim),
    ];
    popup(f, area, look, "Help", lines);
}

fn draw_confirm(f: &mut Frame, area: Rect, look: &Look) {
    let p = &look.pal;
    let lines = vec![
        Line::raw(""),
        Line::raw("  Restart the session from the beginning?"),
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("[Y]", p.accent),
            Span::raw(" Restart    "),
            Span::styled("[N]", p.accent),
            Span::styled(" Cancel", p.dim),
        ]),
    ];
    popup(f, area, look, "Restart", lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn look(unicode: bool) -> Look {
        Look {
            unicode,
            pal: Palette::new(Theme::Minimal, ColorDepth::True),
        }
    }

    fn render(w: u16, h: u16, unicode: bool, f: impl Fn(&mut App, Instant)) -> String {
        let now = Instant::now();
        let s = Settings {
            notifications: false,
            ..Settings::default()
        };
        let mut app = App::new(&s, now, false);
        f(&mut app, now);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|fr| draw(fr, &app, now, &look(unicode))).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn big_font_dimensions() {
        let rows = big_text("24:36", true);
        assert_eq!(rows.len(), 5);
        for r in &rows {
            assert_eq!(r.chars().count(), big_width("24:36"));
        }
        assert_eq!(big_text("8", false)[0], "######");
    }

    #[test]
    fn bar_widths_are_exact() {
        for unicode in [true, false] {
            for i in 0..=100 {
                let (a, b) = progress_bar(i as f64 / 100.0, 37, unicode);
                assert_eq!(a.chars().count() + b.chars().count(), 37, "{i}% {unicode}");
            }
        }
        assert_eq!(progress_bar(1.0, 4, true).0, "████");
        assert_eq!(progress_bar(0.0, 4, false).1, "----");
    }

    #[test]
    fn full_layout_renders_key_elements() {
        let s = render(80, 30, true, |_, _| {});
        assert!(s.contains("W O R K"), "{s}");
        assert!(s.contains("FOCUS SESSION"));
        assert!(s.contains("██████"), "big digits expected");
        assert!(s.contains("White Noise"));
        assert!(s.contains("Volume 35%"));
        assert!(s.contains("[Space] Pause"));
        assert!(s.contains("╭") && s.contains("╯"));
    }

    #[test]
    fn small_terminals_degrade_gracefully() {
        let s = render(40, 14, true, |_, _| {});
        assert!(s.contains("30:00"), "compact timer\n{s}");
        let s = render(12, 3, true, |_, _| {});
        assert!(s.contains("30:00"), "tiny\n{s}");
        // Must not panic on degenerate sizes.
        render(1, 1, true, |_, _| {});
        render(0, 0, true, |_, _| {});
    }

    #[test]
    fn ascii_mode_has_no_unicode() {
        let s = render(80, 30, false, |_, _| {});
        assert!(s.is_ascii(), "{s}");
        assert!(s.contains("[Space] Pause"));
    }

    #[test]
    fn overlays_and_states() {
        use crate::app::Action;
        let s = render(80, 30, true, |a, t| a.handle(Action::ToggleHelp, t));
        assert!(s.contains("Press any key to close"));
        let s = render(80, 30, true, |a, t| a.handle(Action::Restart, t));
        assert!(s.contains("Restart the session"));
        let s = render(80, 30, true, |a, t| a.handle(Action::TogglePause, t));
        assert!(s.contains("PAUSED") && s.contains("[Space] Resume"));
        let s = render(80, 30, true, |a, t| a.handle(Action::ToggleMute, t));
        assert!(s.contains("Muted"));
    }
}
