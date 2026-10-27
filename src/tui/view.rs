//! Rendering for the Vim remote.

use std::sync::OnceLock;

use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{excmd, App, AppMatch, Focus, Mode, Tone};
use crate::companion::keys::PowerState;

#[derive(Debug, Clone, Copy)]
struct Theme {
    accent: Color,
    accent2: Color,
    text: Color,
    dim: Color,
    ok: Color,
    warn: Color,
    err: Color,
    edge: Color,
    colored: bool,
}

impl Theme {
    fn detect() -> Theme {
        if std::env::var_os("NO_COLOR").is_some() {
            return Theme {
                accent: Color::Reset,
                accent2: Color::Reset,
                text: Color::Reset,
                dim: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                err: Color::Reset,
                edge: Color::Reset,
                colored: false,
            };
        }
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let rgb = colorterm.contains("truecolor")
            || colorterm.contains("24bit")
            || term.contains("direct")
            || term.contains("kitty")
            || term.contains("wezterm")
            || term.contains("ghostty");
        if rgb {
            return Theme {
                accent: Color::Rgb(90, 200, 250),
                accent2: Color::Rgb(191, 90, 242),
                text: Color::Rgb(229, 229, 234),
                dim: Color::Rgb(142, 142, 147),
                ok: Color::Rgb(50, 215, 75),
                warn: Color::Rgb(255, 214, 10),
                err: Color::Rgb(255, 69, 58),
                edge: Color::Rgb(58, 58, 60),
                colored: true,
            };
        }
        if term.contains("256") {
            return Theme {
                accent: Color::Indexed(75),
                accent2: Color::Indexed(141),
                text: Color::Indexed(255),
                dim: Color::Indexed(244),
                ok: Color::Indexed(77),
                warn: Color::Indexed(220),
                err: Color::Indexed(203),
                edge: Color::Indexed(238),
                colored: true,
            };
        }
        Theme {
            accent: Color::Cyan,
            accent2: Color::Magenta,
            text: Color::White,
            dim: Color::DarkGray,
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
            edge: Color::DarkGray,
            colored: true,
        }
    }

    fn badge(&self, label: &str, color: Color) -> Span<'static> {
        if self.colored {
            Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" {label} "),
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            )
        }
    }

    fn border(&self) -> Style {
        Style::default().fg(self.edge)
    }

    fn dim(&self) -> Style {
        Style::default().fg(self.dim)
    }
}

fn theme() -> Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    *THEME.get_or_init(Theme::detect)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let theme = theme();
    let area = frame.area();
    if area.width < 50 || area.height < 16 {
        draw_too_small(frame, area, &theme);
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, rows[0], &theme);
    draw_body(frame, app, rows[1], &theme);
    draw_status(frame, app, rows[2], &theme);
    draw_prompt(frame, app, rows[3], &theme);

    if app.mode == Mode::Apps {
        draw_palette(frame, app, &theme);
    }
    if app.mode == Mode::Help {
        frame.render_widget(Clear, frame.area());
        draw_help(frame, &theme);
    }
}

fn draw_too_small(frame: &mut Frame, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(Span::styled(
            "clicker",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "the window is too small for the remote",
            Style::default().fg(theme.text),
        )),
        Line::from(Span::styled(
            format!("{}×{} · needs 50×16", area.width, area.height),
            theme.dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border())
        .title(Line::from(vec![
            Span::styled(
                " clicker ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("· vim remote ", theme.dim()),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut left = vec![Span::styled(
        app.device.clone(),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    if let Some(version) = &app.os_version {
        left.push(Span::styled(format!("  tvOS {version}"), theme.dim()));
    }

    let mut right = vec![power_span(app.power, theme), Span::raw("   ")];
    right.extend(volume_spans(app.volume, theme, 10));

    frame.render_widget(Paragraph::new(spread(left, right, inner.width)), inner);
}

fn power_span(power: PowerState, theme: &Theme) -> Span<'static> {
    let (dot, label, color) = match power {
        PowerState::On => ("●", "on", theme.ok),
        PowerState::Off => ("○", "off", theme.dim),
        PowerState::Unknown => ("○", "—", theme.warn),
    };
    Span::styled(format!("{dot} {label}"), Style::default().fg(color))
}

fn volume_spans(volume: Option<f64>, theme: &Theme, cells: usize) -> Vec<Span<'static>> {
    let Some(level) = volume else {
        return vec![Span::styled("vol —", theme.dim())];
    };
    let filled = (((level.clamp(0.0, 100.0) / 100.0) * cells as f64).round() as usize).min(cells);
    vec![
        Span::styled("vol ", theme.dim()),
        Span::styled("█".repeat(filled), Style::default().fg(theme.accent)),
        Span::styled("░".repeat(cells - filled), theme.dim()),
        Span::styled(format!(" {level:>3.0}%"), Style::default().fg(theme.text)),
    ]
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    if area.width >= 76 {
        let pad_width = if area.width >= 100 { 44 } else { 38 };
        let columns =
            Layout::horizontal([Constraint::Length(pad_width), Constraint::Fill(1)]).split(area);
        draw_pad(frame, app, columns[0], theme);
        draw_sidebar(frame, app, columns[1], theme);
    } else {
        draw_pad(frame, app, area, theme);
    }
}

fn draw_pad(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border())
        .title(Line::from(Span::styled(
            " remote ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(inner);
    let up = centered(rows[0], 52);
    chip(frame, up, "▲ up", "k", app.focus == Focus::Up, theme);

    let middle = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .split(rows[1]);
    chip(
        frame,
        middle[0],
        "◀ left",
        "h",
        app.focus == Focus::Left,
        theme,
    );
    chip(
        frame,
        middle[1],
        "OK",
        "⏎",
        app.focus == Focus::Select,
        theme,
    );
    chip(
        frame,
        middle[2],
        "▶ right",
        "l",
        app.focus == Focus::Right,
        theme,
    );
    chip(
        frame,
        centered(rows[2], 52),
        "▼ down",
        "j",
        app.focus == Focus::Down,
        theme,
    );

    if inner.height >= 12 {
        let action_rows = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);
        let first = Layout::horizontal([Constraint::Percentage(25); 4]).split(action_rows[1]);
        pill(
            frame,
            first[0],
            "⏎",
            "select",
            app.focus == Focus::Select,
            theme,
        );
        pill(
            frame,
            first[1],
            "m",
            "back",
            app.focus == Focus::Back,
            theme,
        );
        pill(
            frame,
            first[2],
            "P",
            "play",
            app.focus == Focus::Play,
            theme,
        );
        pill(
            frame,
            first[3],
            "i",
            "text",
            app.focus == Focus::Text,
            theme,
        );
        let second = Layout::horizontal([Constraint::Percentage(25); 4]).split(action_rows[2]);
        pill(
            frame,
            second[0],
            "gg",
            "home",
            app.focus == Focus::Home,
            theme,
        );
        pill(
            frame,
            second[1],
            "p",
            "power",
            app.focus == Focus::Power,
            theme,
        );
        pill(
            frame,
            second[2],
            "o",
            "apps",
            app.focus == Focus::Apps,
            theme,
        );
        pill(
            frame,
            second[3],
            "?",
            "keys",
            app.focus == Focus::Help,
            theme,
        );
    }
}

fn chip(frame: &mut Frame, area: Rect, label: &str, hint: &str, active: bool, theme: &Theme) {
    let border = if active {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.edge)
    };
    let content = if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let line = Line::from(vec![
        Span::styled(label.to_string(), content),
        Span::styled(format!(" {hint}"), theme.dim()),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border),
        ),
        area,
    );
}

fn pill(frame: &mut Frame, area: Rect, hint: &str, label: &str, active: bool, theme: &Theme) {
    let hint_style = Style::default()
        .fg(if active { theme.accent } else { theme.accent2 })
        .add_modifier(Modifier::BOLD);
    let label_style = if active {
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        theme.dim()
    };
    let line = Line::from(vec![
        Span::styled(format!("{hint} "), hint_style),
        Span::styled(label.to_string(), label_style),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(3)]).split(area);
    draw_now(frame, app, rows[0], theme);
    draw_activity(frame, app, rows[1], theme);
}

fn draw_now(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border())
        .title(Line::from(Span::styled(
            " now ",
            Style::default()
                .fg(theme.accent2)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let connection = Line::from(vec![
        Span::styled("● connected", Style::default().fg(theme.ok)),
        Span::styled("  companion", theme.dim()),
    ]);
    let platform = match (&app.os_version, &app.model) {
        (Some(version), Some(model)) => format!("tvOS {version} · {model}"),
        (Some(version), None) => format!("tvOS {version}"),
        (None, Some(model)) => model.clone(),
        (None, None) => "tvOS —".into(),
    };
    let last = Line::from(vec![
        Span::styled("last ", theme.dim()),
        Span::styled(
            match &app.last_app {
                Some(bundle) => app.app_name(bundle),
                None => "no app yet".into(),
            },
            Style::default().fg(theme.text),
        ),
    ]);
    let mut details = vec![
        connection,
        Line::from(Span::styled(
            app.device.clone(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(platform, theme.dim())),
        Line::from(volume_spans(app.volume, theme, 8)),
        last,
    ];
    details.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(details), inner);
}

fn draw_activity(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border())
        .title(Line::from(Span::styled(
            " activity ",
            Style::default()
                .fg(theme.accent2)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    if app.activity.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("nothing yet", theme.dim()))),
            inner,
        );
        return;
    }

    let lines: Vec<Line> = app
        .activity
        .iter()
        .take(inner.height as usize)
        .map(|entry| {
            Line::from(vec![
                Span::styled("▸ ", theme.dim()),
                Span::styled(
                    match entry.count {
                        1 => entry.text.clone(),
                        count => format!("{} ×{count}", entry.text),
                    },
                    Style::default().fg(tone_color(entry.tone, theme)),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn tone_color(tone: Tone, theme: &Theme) -> Color {
    match tone {
        Tone::Info => theme.dim,
        Tone::Ok => theme.ok,
        Tone::Err => theme.err,
    }
}

fn mode_color(mode: Mode, theme: &Theme) -> Color {
    match mode {
        Mode::Normal => theme.ok,
        Mode::Command => theme.accent,
        Mode::Insert { .. } => theme.warn,
        Mode::Apps => theme.accent2,
        Mode::Help => theme.accent,
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let mut left = vec![theme.badge(app.mode.name(), mode_color(app.mode, theme))];
    let pending = app.parser.pending();
    if !pending.is_empty() {
        left.push(Span::styled(
            format!(" {pending}"),
            Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
        ));
    }
    if matches!(app.mode, Mode::Insert { .. }) {
        left.push(Span::styled("  ctrl-c or esc to leave", theme.dim()));
    }
    let right = if let Some(register) = app.recording {
        vec![Span::styled(
            format!("● recording @{register}"),
            Style::default().fg(theme.err).add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::styled("? keys   :q quit", theme.dim())]
    };
    frame.render_widget(Paragraph::new(spread(left, right, area.width)), area);
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    match app.mode {
        Mode::Command => {
            let text = app.line.text();
            let ghost = completion_ghost(app, &text);
            let line = Line::from(vec![
                Span::styled(
                    ":",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(text, Style::default().fg(theme.text)),
                Span::styled(ghost, theme.dim()),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            place_cursor(frame, area, 1 + width(&app.line.prefix()));
        }
        Mode::Insert { append } => {
            let verb = if append { "append" } else { "type" };
            let line = Line::from(vec![
                Span::styled(
                    format!("{verb} ▸ "),
                    Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
                ),
                Span::styled(app.line.text(), Style::default().fg(theme.text)),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            place_cursor(
                frame,
                area,
                width(&format!("{verb} ▸ ")) + width(&app.line.prefix()),
            );
        }
        Mode::Apps => {
            let style = Style::default().fg(tone_color(app.message.tone, theme));
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("∙ ", theme.dim()),
                    Span::styled(app.message.text.clone(), style),
                ])),
                area,
            );
        }
        Mode::Help => {}
        Mode::Normal => {
            let style = Style::default().fg(tone_color(app.message.tone, theme));
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("∙ ", theme.dim()),
                    Span::styled(app.message.text.clone(), style),
                ])),
                area,
            );
        }
    }
}

fn completion_ghost(app: &App, text: &str) -> String {
    if !app.line.at_end() {
        return String::new();
    }
    let Some(done) = excmd::complete(text, &app.names()) else {
        return String::new();
    };
    done.strip_prefix(text).unwrap_or_default().to_string()
}

fn place_cursor(frame: &mut Frame, area: Rect, offset: usize) {
    let x = area.x.saturating_add(offset.min(u16::MAX as usize) as u16);
    let x = x.min(area.x + area.width.saturating_sub(1));
    frame.set_cursor_position(Position::new(x, area.y));
}

fn draw_palette(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = popup(frame.area(), 64, 18);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent2))
        .title(Line::from(vec![
            Span::styled(
                " apps ",
                Style::default()
                    .fg(theme.accent2)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("· launch something ", theme.dim()),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 {
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⌕ ", Style::default().fg(theme.accent2)),
            Span::styled(app.palette.query.text(), Style::default().fg(theme.text)),
        ])),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            theme.dim(),
        ))),
        rows[1],
    );
    place_cursor(frame, rows[0], 2 + width(&app.palette.query.prefix()));

    if app.palette.loading {
        centered_message(frame, rows[2], "loading the app list…", theme);
    } else if let Some(error) = &app.palette.error {
        centered_message(frame, rows[2], error, theme);
    } else if app.palette.list.is_empty() {
        let message = if app.palette.query.is_empty() {
            "this Apple TV did not report any apps"
        } else {
            "no apps match"
        };
        centered_message(frame, rows[2], message, theme);
    } else {
        draw_results(frame, app, rows[2], theme);
    }

    let count = app.palette.list.len();
    let total = app.apps.len();
    let left = vec![
        Span::styled(
            "enter ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("launch   ", theme.dim()),
        Span::styled(
            "↑↓ ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("choose   ", theme.dim()),
        Span::styled(
            "esc ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("close", theme.dim()),
    ];
    let right = vec![Span::styled(
        if app.palette.loading {
            "…".to_string()
        } else {
            format!("{count} of {total}")
        },
        theme.dim(),
    )];
    frame.render_widget(Paragraph::new(spread(left, right, inner.width)), rows[3]);
}

fn draw_results(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let visible = area.height as usize;
    let selected = app.palette.selected.min(app.palette.list.len() - 1);
    let offset = if app.palette.list.len() <= visible {
        0
    } else {
        selected
            .saturating_sub(visible / 2)
            .min(app.palette.list.len() - visible)
    };

    for (row, item) in app
        .palette
        .list
        .iter()
        .skip(offset)
        .take(visible)
        .enumerate()
    {
        let index = offset + row;
        let line = result_line(item, index == selected, area.width, theme);
        frame.render_widget(
            Paragraph::new(line),
            Rect {
                x: area.x,
                y: area.y + row as u16,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn result_line(item: &AppMatch, selected: bool, width: u16, theme: &Theme) -> Line<'static> {
    let base = if selected && theme.colored {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent2)
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let hit = if selected {
        base.add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };

    let marker = if selected { "▸ " } else { "  " };
    let mut left = vec![Span::styled(marker.to_string(), base)];
    left.extend(marked(&item.name, &item.hit.positions, base, hit));
    let right = vec![Span::styled(
        item.bundle.clone(),
        if selected { base } else { theme.dim() },
    )];
    spread(left, right, width)
}

fn marked(text: &str, positions: &[usize], base: Style, hit: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chunk = String::new();
    let mut chunk_hit = false;
    for (index, c) in text.chars().enumerate() {
        let is_hit = positions.binary_search(&index).is_ok();
        if is_hit != chunk_hit && !chunk.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut chunk),
                if chunk_hit { hit } else { base },
            ));
        }
        chunk_hit = is_hit;
        chunk.push(c);
    }
    if !chunk.is_empty() {
        spans.push(Span::styled(chunk, if chunk_hit { hit } else { base }));
    }
    spans
}

fn centered_message(frame: &mut Frame, area: Rect, message: &str, theme: &Theme) {
    let row = Rect {
        x: area.x,
        y: area.y + area.height / 2,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(message.to_string(), theme.dim())))
            .alignment(Alignment::Center),
        row,
    );
}

fn draw_help(frame: &mut Frame, theme: &Theme) {
    let area = popup(frame.area(), 78, frame.area().height);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Line::from(vec![
            Span::styled(
                " keys ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("· vim mode ", theme.dim()),
        ]))
        .title_bottom(
            Line::from(Span::styled(" press any key to go back ", theme.dim())).right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 6 || inner.width < 40 {
        return;
    }

    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);

    let left = vec![
        section("MOTIONS", theme),
        key_line("h j k l", "arrows", theme),
        key_line("gg  H", "home", theme),
        key_line("G", "last app", theme),
        key_line("w  b", "skip ±10s", theme),
        key_line("J  K", "hold down, hold up", theme),
        key_line("ctrl-d/u", "page down/up", theme),
        key_line("[  ]", "channel down/up", theme),
        Line::default(),
        section("REMOTE", theme),
        key_line("enter", "select", theme),
        key_line("m", "back", theme),
        key_line("P", "play/pause", theme),
        key_line("p", "power", theme),
        key_line("s", "siri", theme),
        key_line("c", "click (2c double)", theme),
        key_line("> <", "volume", theme),
        key_line("r", "refresh", theme),
        Line::default(),
        section("SWIPES", theme),
        key_line(":swipe up", "or down · left · right", theme),
    ];

    let right = vec![
        section("MODES", theme),
        key_line("i  t", "type text", theme),
        key_line("a", "append text", theme),
        key_line("o  /", "apps", theme),
        key_line(":", "command", theme),
        key_line("?", "this help", theme),
        Line::default(),
        section("MACROS", theme),
        key_line("q a", "record @a", theme),
        key_line("@ a", "play @a", theme),
        key_line("@ @", "play the last one", theme),
        key_line("@ :", "repeat :command", theme),
        key_line(".", "repeat last action", theme),
        key_line("Z Z", "quit", theme),
        Line::default(),
        section("COMMANDS", theme),
        key_line(":q", ":power on · :vol 40", theme),
        key_line(":seek -15", ":app net", theme),
        key_line(":text hi", ":open url", theme),
        key_line(":page up", ":account me", theme),
        key_line(":siri", ":guide · :sleep", theme),
    ];

    frame.render_widget(Paragraph::new(left), columns[0]);
    frame.render_widget(Paragraph::new(right), columns[1]);
}

fn section(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(theme.accent2)
            .add_modifier(Modifier::BOLD),
    ))
}

fn key_line(keys: &str, description: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {keys:<12}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(description.to_string(), Style::default().fg(theme.text)),
    ])
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(20);
    let height = height.min(area.height.saturating_sub(1)).max(8);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn centered(area: Rect, percent: u16) -> Rect {
    let columns = Layout::horizontal([
        Constraint::Percentage((100 - percent) / 2),
        Constraint::Percentage(percent),
        Constraint::Percentage((100 - percent) / 2),
    ])
    .split(area);
    columns[1]
}

fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let left_width = spans_width(&left);
    let right_width = spans_width(&right);
    let usable = width.saturating_sub(2) as usize;
    if left_width + right_width <= usable {
        let gap = usable - left_width - right_width;
        let mut spans = vec![Span::raw(" ")];
        spans.extend(left);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right);
        spans.push(Span::raw(" "));
        return Line::from(spans);
    }
    Line::from(left)
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[cfg(test)]
mod tests {
    use super::{popup, spread, width};
    use ratatui::layout::Rect;
    use ratatui::text::Span;

    #[test]
    fn spread_fills_the_exact_width() {
        let line = spread(vec![Span::raw("left")], vec![Span::raw("right")], 30);
        let text: String = line.spans.iter().map(|span| span.content.clone()).collect();
        assert_eq!(text.chars().count(), 30);
        assert!(text.starts_with(" left"));
        assert!(text.ends_with("right "));
    }

    #[test]
    fn spread_keeps_the_left_side_when_cramped() {
        let line = spread(vec![Span::raw("left")], vec![Span::raw("right")], 8);
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn popups_stay_inside_the_screen() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        };
        let rect = popup(area, 64, 18);
        assert!(rect.width <= area.width);
        assert!(rect.height <= area.height);
    }

    #[test]
    fn width_counts_double_width_characters() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本"), 4);
    }
}
