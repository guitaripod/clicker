//! Interactive on-screen remote.

use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use crate::Result;

use crate::companion::client::Client;
use crate::companion::keys::{HidCommand, InputAction, PowerState};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Up,
    Left,
    Select,
    Right,
    Down,
    PlayPause,
    Home,
    Menu,
    Power,
}

impl Focus {
    const GRID: [[Focus; 3]; 3] = [
        [Focus::Home, Focus::Up, Focus::Power],
        [Focus::Left, Focus::Select, Focus::Right],
        [Focus::Menu, Focus::Down, Focus::PlayPause],
    ];

    fn label(self) -> &'static str {
        match self {
            Focus::Up => "▲",
            Focus::Down => "▼",
            Focus::Left => "◀",
            Focus::Right => "▶",
            Focus::Select => "OK",
            Focus::Home => "TV",
            Focus::Menu => "Back",
            Focus::Power => "⏻",
            Focus::PlayPause => "▶︎Ⅱ",
        }
    }

    fn command(self) -> Option<HidCommand> {
        Some(match self {
            Focus::Up => HidCommand::Up,
            Focus::Down => HidCommand::Down,
            Focus::Left => HidCommand::Left,
            Focus::Right => HidCommand::Right,
            Focus::Select => HidCommand::Select,
            Focus::Home => HidCommand::Home,
            Focus::Menu => HidCommand::Menu,
            Focus::PlayPause => HidCommand::PlayPause,
            Focus::Power => HidCommand::Sleep,
        })
    }
}

enum Mode {
    Navigate,
    Typing(String),
}

struct App {
    focus: Focus,
    mode: Mode,
    power: PowerState,
    volume: Option<f64>,
    status: String,
    device: String,
}

pub async fn run(client: Client, device: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &client, device).await;

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

async fn event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    client: &Client,
    device: &str,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(32);
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(event) => {
                if tx.blocking_send(event).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    });

    let mut app = App {
        focus: Focus::Select,
        mode: Mode::Navigate,
        power: PowerState::Unknown,
        volume: None,
        status: String::new(),
        device: device.to_string(),
    };
    let mut device_events = client.events();
    let mut tick = tokio::time::interval(Duration::from_millis(400));

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        tokio::select! {
            received = rx.recv() => {
                let Some(TermEvent::Key(key)) = received else { continue };
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if handle_key(client, &mut app, key).await? {
                    return Ok(());
                }
            }
            event = device_events.recv() => {
                if let Ok(event) = event {
                    match event.name.as_str() {
                        "SystemStatus" | "TVSystemStatus" => {
                            if let Some(state) = event.content.get("state").and_then(|v| v.as_i64()) {
                                app.power = PowerState::from_attention_state(state);
                            }
                        }
                        "_iMC" => {
                            if let Some(level) = refresh_volume(client).await {
                                app.volume = Some(level);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = tick.tick() => {
                app.power = client.power_state().await;
                if let Some(level) = refresh_volume(client).await {
                    app.volume = Some(level);
                }
            }
        }
    }
}

async fn refresh_volume(client: &Client) -> Option<f64> {
    client.volume().await.ok().flatten()
}

async fn handle_key(client: &Client, app: &mut App, key: KeyEvent) -> Result<bool> {
    if let Mode::Typing(buffer) = &mut app.mode {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Navigate;
                app.status = "text input cancelled".into();
            }
            KeyCode::Enter => {
                let text = buffer.clone();
                match client.text_set(&text).await {
                    Ok(value) => app.status = format!("typed: {value}"),
                    Err(err) => app.status = err.to_string(),
                }
                app.mode = Mode::Navigate;
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(c) => buffer.push(c),
            _ => {}
        }
        return Ok(false);
    }

    let activate = matches!(key.code, KeyCode::Enter | KeyCode::Char(' '));
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        KeyCode::Left => app.focus = neighbour(app.focus, 0, -1),
        KeyCode::Right => app.focus = neighbour(app.focus, 0, 1),
        KeyCode::Up => app.focus = neighbour(app.focus, -1, 0),
        KeyCode::Down => app.focus = neighbour(app.focus, 1, 0),
        KeyCode::Char('+') | KeyCode::Char('=') => {
            press(client, app, HidCommand::VolumeUp, false).await;
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            press(client, app, HidCommand::VolumeDown, false).await;
        }
        KeyCode::Char('h') => press(client, app, HidCommand::Home, false).await,
        KeyCode::Char('m') => press(client, app, HidCommand::Menu, false).await,
        KeyCode::Char('t') => app.mode = Mode::Typing(String::new()),
        KeyCode::Char('r') => {
            app.power = client.power_state().await;
            app.volume = refresh_volume(client).await;
            app.status = "refreshed".into();
        }
        KeyCode::Char('p') => {
            let command = if app.power == PowerState::On {
                HidCommand::Sleep
            } else {
                HidCommand::Wake
            };
            press(client, app, command, false).await;
        }
        _ if activate => {
            if let Some(command) = app.focus.command() {
                press(client, app, command, false).await;
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn press(client: &Client, app: &mut App, command: HidCommand, hold: bool) {
    let action = if hold {
        InputAction::Hold
    } else {
        InputAction::Single
    };
    match client
        .press(command, action, Duration::from_millis(900))
        .await
    {
        Ok(()) => app.status = format!("sent {}", command.name()),
        Err(err) => app.status = err.to_string(),
    }
}

fn neighbour(focus: Focus, row_delta: i32, col_delta: i32) -> Focus {
    let mut position = (1usize, 1usize);
    for (row, cells) in Focus::GRID.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            if *cell == focus {
                position = (row, col);
            }
        }
    }
    let row = (position.0 as i32 + row_delta).clamp(0, 2) as usize;
    let col = (position.1 as i32 + col_delta).clamp(0, 2) as usize;
    Focus::GRID[row][col]
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(9),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", app.device),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}   volume {}",
                app.power.as_str(),
                match app.volume {
                    Some(level) => format!("{level:.0}%"),
                    None => "external".into(),
                }
            ),
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" clicker "))
    .alignment(Alignment::Left);
    frame.render_widget(title, chunks[0]);

    let grid = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(chunks[1]);
    for (row, cells) in Focus::GRID.iter().enumerate() {
        let columns = Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(grid[row]);
        for (col, focus) in cells.iter().enumerate() {
            let selected = *focus == app.focus;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let widget = Paragraph::new(focus.label())
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Center)
                .style(style);
            frame.render_widget(widget, columns[col]);
        }
    }

    let status = match &app.mode {
        Mode::Typing(buffer) => format!("type text: {buffer}▏"),
        Mode::Navigate => app.status.clone(),
    };
    frame.render_widget(
        Paragraph::new(status).block(Block::default().borders(Borders::ALL).title(" status ")),
        chunks[2],
    );

    let help = match &app.mode {
        Mode::Typing(_) => "enter send   esc cancel".to_string(),
        Mode::Navigate => {
            "arrows move   enter press   h home   m back   p power   +/- volume   t text   r refresh   q quit"
                .to_string()
        }
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
}
