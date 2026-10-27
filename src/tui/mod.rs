//! Vim-driven on-screen remote.

mod excmd;
mod fuzzy;
mod keys;
mod view;

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::companion::client::{Client, Event};
use crate::companion::keys::{Gesture, HidCommand, InputAction, PowerState};
use crate::discovery;
use crate::store::{PairedDevice, Store};
use crate::tui::excmd::Ex;
use crate::tui::fuzzy::Hit;
use crate::tui::keys::{Action, Key, Parsed, Parser};
use crate::{Error, Result};

const HOLD: Duration = Duration::from_millis(900);
const POLL: Duration = Duration::from_millis(1500);
const MACRO_LIMIT: usize = 400;
const ACTIVITY_LIMIT: usize = 12;
const RECENT_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Command,
    Insert { append: bool },
    Apps,
    Help,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Command => "COMMAND",
            Mode::Insert { .. } => "INSERT",
            Mode::Apps => "APPS",
            Mode::Help => "HELP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Ok,
    Err,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDir {
    Left,
    Right,
    Up,
    Down,
}

impl SwipeDir {
    fn name(self) -> &'static str {
        match self {
            SwipeDir::Left => "left",
            SwipeDir::Right => "right",
            SwipeDir::Up => "up",
            SwipeDir::Down => "down",
        }
    }

    fn gesture(self) -> Gesture {
        let (from, to) = match self {
            SwipeDir::Left => ((800, 500), (200, 500)),
            SwipeDir::Right => ((200, 500), (800, 500)),
            SwipeDir::Up => ((500, 800), (500, 200)),
            SwipeDir::Down => ((500, 200), (500, 800)),
        };
        Gesture {
            from,
            to,
            duration: Duration::from_millis(220),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Up,
    Down,
    Left,
    Right,
    Select,
    Home,
    Back,
    Play,
    Text,
    Power,
    Apps,
    Siri,
    Help,
}

impl Focus {
    fn for_command(command: HidCommand) -> Option<Focus> {
        Some(match command {
            HidCommand::Up => Focus::Up,
            HidCommand::Down => Focus::Down,
            HidCommand::Left => Focus::Left,
            HidCommand::Right => Focus::Right,
            HidCommand::Select => Focus::Select,
            HidCommand::Home => Focus::Home,
            HidCommand::Menu => Focus::Back,
            HidCommand::PlayPause => Focus::Play,
            HidCommand::Screensaver | HidCommand::Sleep | HidCommand::Wake => Focus::Power,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
enum Job {
    Press {
        command: HidCommand,
        times: u32,
        action: InputAction,
    },
    Volume {
        up: bool,
        times: u32,
    },
    SetVolume(f64),
    Seek(f64),
    TogglePower,
    Sleep,
    Wake,
    Siri,
    Click {
        double: bool,
    },
    Swipe(SwipeDir),
    Launch {
        bundle: String,
        name: String,
    },
    OpenUrl(String),
    Account(String),
    Text {
        value: String,
        append: bool,
    },
    LoadApps,
    Refresh,
    Poll,
}

#[derive(Debug)]
struct Outcome {
    message: String,
    tone: Tone,
    silent: bool,
    power: Option<PowerState>,
    volume: Option<Option<f64>>,
    apps: Option<std::result::Result<Vec<(String, String)>, String>>,
    lost: bool,
    unsent: bool,
}

impl Outcome {
    fn ok(message: impl Into<String>) -> Outcome {
        Outcome {
            message: message.into(),
            tone: Tone::Ok,
            silent: false,
            power: None,
            volume: None,
            apps: None,
            lost: false,
            unsent: false,
        }
    }

    fn err(error: Error) -> Outcome {
        Outcome {
            message: error.to_string(),
            tone: Tone::Err,
            silent: false,
            power: None,
            volume: None,
            apps: None,
            lost: is_lost(&error),
            unsent: never_sent(&error),
        }
    }

    fn note(message: impl Into<String>, tone: Tone) -> Outcome {
        Outcome {
            message: message.into(),
            tone,
            silent: false,
            power: None,
            volume: None,
            apps: None,
            lost: false,
            unsent: false,
        }
    }

    fn quiet() -> Outcome {
        Outcome {
            message: String::new(),
            tone: Tone::Info,
            silent: true,
            power: None,
            volume: None,
            apps: None,
            lost: false,
            unsent: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Message {
    text: String,
    tone: Tone,
}

#[derive(Debug, Clone)]
struct Activity {
    text: String,
    tone: Tone,
    count: u32,
}

#[derive(Debug, Clone)]
enum Change {
    Action { action: Action, count: u32 },
    Macro { register: char, times: u32 },
}

#[derive(Debug, Default, Clone)]
struct Line {
    chars: Vec<char>,
    cursor: usize,
}

impl Line {
    fn set(&mut self, text: String) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    fn text(&self) -> String {
        self.chars.iter().collect()
    }

    fn prefix(&self) -> String {
        self.chars[..self.cursor].iter().collect()
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    fn at_end(&self) -> bool {
        self.cursor == self.chars.len()
    }

    fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    fn kill_to_start(&mut self) {
        self.chars.drain(..self.cursor);
        self.cursor = 0;
    }

    fn kill_to_end(&mut self) {
        self.chars.truncate(self.cursor);
    }

    fn kill_word(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
    }

    fn edit(&mut self, key: Key) -> bool {
        match key {
            Key::Char(c) => {
                self.insert(c);
                true
            }
            Key::Backspace => {
                self.backspace();
                true
            }
            Key::Delete => {
                self.delete();
                true
            }
            Key::Left => {
                self.left();
                false
            }
            Key::Right => {
                self.right();
                false
            }
            Key::Home | Key::Ctrl('a') => {
                self.home();
                false
            }
            Key::End | Key::Ctrl('e') => {
                self.end();
                false
            }
            Key::Ctrl('u') => {
                self.kill_to_start();
                true
            }
            Key::Ctrl('k') => {
                self.kill_to_end();
                true
            }
            Key::Ctrl('w') => {
                self.kill_word();
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
struct AppMatch {
    bundle: String,
    name: String,
    hit: Hit,
}

#[derive(Debug, Default)]
struct Palette {
    query: Line,
    list: Vec<AppMatch>,
    selected: usize,
    loading: bool,
    error: Option<String>,
}

impl Palette {
    fn open(&mut self, query: String) {
        self.query.set(query);
        self.list.clear();
        self.selected = 0;
        self.error = None;
    }

    fn refilter(&mut self, apps: &[(String, String)], recents: &[String], reset: bool) {
        let query = self.query.text();
        let query = query.trim();
        let mut list = Vec::new();
        for (bundle, name) in apps {
            let Some(mut hit) = fuzzy::score(query, name) else {
                continue;
            };
            if recents.iter().any(|recent| recent == bundle) {
                hit.score += 6;
            }
            list.push(AppMatch {
                bundle: bundle.clone(),
                name: name.clone(),
                hit,
            });
        }
        list.sort_by(|a, b| {
            b.hit
                .score
                .cmp(&a.hit.score)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.list = list;
        if reset {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.list.len().saturating_sub(1));
        }
    }

    fn move_by(&mut self, delta: i32) {
        if self.list.is_empty() {
            return;
        }
        let len = self.list.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }

    fn current(&self) -> Option<&AppMatch> {
        self.list.get(self.selected)
    }
}

struct App {
    device: String,
    os_version: Option<String>,
    model: Option<String>,
    power: PowerState,
    volume: Option<f64>,
    mode: Mode,
    focus: Focus,
    parser: Parser,
    change: Option<Change>,
    macros: HashMap<char, Vec<Key>>,
    recording: Option<char>,
    recorded: Vec<Key>,
    replay_depth: u8,
    last_macro: Option<char>,
    last_ex: Option<String>,
    line: Line,
    history: Vec<String>,
    history_at: Option<usize>,
    palette: Palette,
    apps: Vec<(String, String)>,
    apps_loaded: bool,
    recents: Vec<String>,
    last_app: Option<String>,
    activity: VecDeque<Activity>,
    message: Message,
    quit: bool,
}

impl App {
    fn new(device: &str, os_version: Option<String>, model: Option<String>) -> App {
        App {
            device: device.to_string(),
            os_version,
            model,
            power: PowerState::Unknown,
            volume: None,
            mode: Mode::Normal,
            focus: Focus::Select,
            parser: Parser::default(),
            change: None,
            macros: HashMap::new(),
            recording: None,
            recorded: Vec::new(),
            replay_depth: 0,
            last_macro: None,
            last_ex: None,
            line: Line::default(),
            history: Vec::new(),
            history_at: None,
            palette: Palette::default(),
            apps: Vec::new(),
            apps_loaded: false,
            recents: Vec::new(),
            last_app: None,
            activity: VecDeque::new(),
            message: Message {
                text: "press ? for keys · :q quits".into(),
                tone: Tone::Info,
            },
            quit: false,
        }
    }

    fn key(&mut self, key: Key) -> Vec<Job> {
        let mut jobs = Vec::new();
        if key == Key::Ctrl('c') {
            if self.mode == Mode::Normal {
                if self.parser.idle() {
                    self.quit = true;
                } else {
                    self.parser.clear();
                }
            } else {
                self.mode = Mode::Normal;
                self.stop_recording();
            }
            return jobs;
        }
        let stops_recording =
            self.mode == Mode::Normal && self.parser.idle() && key == Key::Char('q');
        if self.recording.is_some()
            && self.replay_depth == 0
            && !stops_recording
            && self.recorded.len() < MACRO_LIMIT
        {
            self.recorded.push(key);
        }
        match self.mode {
            Mode::Normal => self.normal(key, &mut jobs),
            Mode::Command => self.command(key, &mut jobs),
            Mode::Insert { append } => self.insert(key, append, &mut jobs),
            Mode::Apps => self.apps(key, &mut jobs),
            Mode::Help => self.mode = Mode::Normal,
        }
        jobs
    }

    fn normal(&mut self, key: Key, jobs: &mut Vec<Job>) {
        match self.parser.feed(key, self.recording.is_some()) {
            Parsed::Nothing | Parsed::Pending => {}
            Parsed::Run(action, count) => {
                self.change = Some(Change::Action {
                    action: action.clone(),
                    count,
                });
                self.execute(action, count, jobs);
            }
            Parsed::Open(mode) => self.enter(mode, jobs),
            Parsed::Record(Some(register)) => self.start_recording(register),
            Parsed::Record(None) => self.stop_recording(),
            Parsed::Play(register, times) => self.play(register, times, jobs),
            Parsed::Repeat(count) => match self.change.clone() {
                Some(Change::Action {
                    action,
                    count: last,
                }) => self.execute(action, count.unwrap_or(last), jobs),
                Some(Change::Macro { register, times }) => {
                    self.play(Some(register), count.unwrap_or(times), jobs)
                }
                None => self.say("there is nothing to repeat yet", Tone::Info),
            },
            Parsed::Quit => self.quit = true,
        }
    }

    fn command(&mut self, key: Key, jobs: &mut Vec<Job>) {
        match key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Enter => {
                let line = self.line.text();
                self.mode = Mode::Normal;
                if line.trim().is_empty() {
                    return;
                }
                if self.history.last() != Some(&line) {
                    self.history.push(line.clone());
                }
                self.history_at = None;
                self.last_ex = Some(line.clone());
                self.run_line(&line, jobs);
            }
            Key::Tab => {
                let names = self.names();
                if let Some(done) = excmd::complete(&self.line.text(), &names) {
                    self.line.set(done);
                }
            }
            Key::Up => self.history_move(-1),
            Key::Down => self.history_move(1),
            other => {
                self.line.edit(other);
            }
        }
    }

    fn insert(&mut self, key: Key, append: bool, jobs: &mut Vec<Job>) {
        match key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Enter => {
                let value = self.line.text();
                self.mode = Mode::Normal;
                if value.trim().is_empty() {
                    return;
                }
                jobs.push(Job::Text { value, append });
            }
            other => {
                self.line.edit(other);
            }
        }
    }

    fn apps(&mut self, key: Key, jobs: &mut Vec<Job>) {
        match key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Up | Key::Ctrl('p') => self.palette.move_by(-1),
            Key::Down | Key::Ctrl('n') => self.palette.move_by(1),
            Key::PageUp => self.palette.move_by(-5),
            Key::PageDown => self.palette.move_by(5),
            Key::Enter => {
                let Some(selected) = self.palette.current().cloned() else {
                    return;
                };
                self.mode = Mode::Normal;
                self.focus = Focus::Apps;
                self.remember(&selected.bundle);
                jobs.push(Job::Launch {
                    bundle: selected.bundle,
                    name: selected.name,
                });
            }
            other => {
                if self.palette.query.edit(other) {
                    self.palette.refilter(&self.apps, &self.recents, true);
                }
            }
        }
    }

    fn enter(&mut self, mode: Mode, jobs: &mut Vec<Job>) {
        match mode {
            Mode::Command => {
                self.mode = Mode::Command;
                self.line.clear();
                self.history_at = None;
            }
            Mode::Insert { append } => {
                self.mode = Mode::Insert { append };
                self.line.clear();
                self.focus = Focus::Text;
            }
            Mode::Apps => self.open_apps(String::new(), jobs),
            Mode::Help => {
                self.mode = Mode::Help;
                self.focus = Focus::Help;
            }
            Mode::Normal => self.mode = Mode::Normal,
        }
    }

    fn open_apps(&mut self, query: String, jobs: &mut Vec<Job>) {
        self.mode = Mode::Apps;
        self.focus = Focus::Apps;
        self.palette.open(query);
        self.palette.refilter(&self.apps, &self.recents, true);
        if !self.apps_loaded && !self.palette.loading {
            self.palette.loading = true;
            jobs.push(Job::LoadApps);
        }
    }

    fn execute(&mut self, action: Action, count: u32, jobs: &mut Vec<Job>) {
        match action {
            Action::Press(command) => {
                if let Some(focus) = Focus::for_command(command) {
                    self.focus = focus;
                }
                jobs.push(Job::Press {
                    command,
                    times: count,
                    action: InputAction::Single,
                });
            }
            Action::Hold(command) => {
                if let Some(focus) = Focus::for_command(command) {
                    self.focus = focus;
                }
                jobs.push(Job::Press {
                    command,
                    times: count,
                    action: InputAction::Hold,
                });
            }
            Action::Volume(direction) => jobs.push(Job::Volume {
                up: direction > 0,
                times: count,
            }),
            Action::Seek(seconds) => jobs.push(Job::Seek(seconds * count as f64)),
            Action::TogglePower => {
                self.focus = Focus::Power;
                jobs.push(Job::TogglePower);
            }
            Action::Siri => {
                self.focus = Focus::Siri;
                jobs.push(Job::Siri);
            }
            Action::Click { double } => jobs.push(Job::Click {
                double: double || count > 1,
            }),
            Action::Refresh => jobs.push(Job::Refresh),
            Action::LastApp => match self.last_app.clone() {
                Some(bundle) => {
                    self.focus = Focus::Apps;
                    let name = self.app_name(&bundle);
                    jobs.push(Job::Launch { bundle, name });
                }
                None => self.say("no app launched yet this session", Tone::Info),
            },
        }
    }

    fn run_line(&mut self, line: &str, jobs: &mut Vec<Job>) {
        match excmd::parse(line) {
            Ok(ex) => self.exe(ex, jobs),
            Err(error) => self.say(error, Tone::Err),
        }
    }

    fn exe(&mut self, ex: Ex, jobs: &mut Vec<Job>) {
        match ex {
            Ex::Noop => {}
            Ex::Quit => self.quit = true,
            Ex::Power(Some(true)) => jobs.push(Job::Wake),
            Ex::Power(Some(false)) => jobs.push(Job::Sleep),
            Ex::Power(None) => jobs.push(Job::TogglePower),
            Ex::Sleep => jobs.push(Job::Sleep),
            Ex::Wake => jobs.push(Job::Wake),
            Ex::VolumeShow => {
                let text = match self.volume {
                    Some(level) => format!("volume {level:.0}%"),
                    None => "volume is controlled by the TV or receiver".into(),
                };
                self.say(text, Tone::Info);
            }
            Ex::VolumeSet(level) => jobs.push(Job::SetVolume(level)),
            Ex::VolumeStep(steps) => jobs.push(Job::Volume {
                up: steps > 0,
                times: steps.unsigned_abs(),
            }),
            Ex::Seek(seconds) => jobs.push(Job::Seek(seconds)),
            Ex::App(query) => self.open_apps(query.unwrap_or_default(), jobs),
            Ex::Apps => self.open_apps(String::new(), jobs),
            Ex::Open(url) => jobs.push(Job::OpenUrl(url)),
            Ex::Account(query) => jobs.push(Job::Account(query)),
            Ex::Siri => {
                self.focus = Focus::Siri;
                jobs.push(Job::Siri);
            }
            Ex::Click(double) => jobs.push(Job::Click { double }),
            Ex::Press(command) => {
                if let Some(focus) = Focus::for_command(command) {
                    self.focus = focus;
                }
                jobs.push(Job::Press {
                    command,
                    times: 1,
                    action: InputAction::Single,
                });
            }
            Ex::Page(down) => jobs.push(Job::Press {
                command: if down {
                    HidCommand::PageDown
                } else {
                    HidCommand::PageUp
                },
                times: 1,
                action: InputAction::Single,
            }),
            Ex::Swipe(direction) => jobs.push(Job::Swipe(direction)),
            Ex::Text(Some(value)) => jobs.push(Job::Text {
                value,
                append: false,
            }),
            Ex::Text(None) => self.enter(Mode::Insert { append: false }, jobs),
            Ex::Refresh => jobs.push(Job::Refresh),
            Ex::Help => self.enter(Mode::Help, jobs),
            Ex::Version => self.say(format!("clicker {}", env!("CARGO_PKG_VERSION")), Tone::Info),
            Ex::Device => {
                let mut detail = self.device.clone();
                if let Some(version) = &self.os_version {
                    detail.push_str(&format!(" · tvOS {version}"));
                }
                if let Some(model) = &self.model {
                    detail.push_str(&format!(" · {model}"));
                }
                self.say(detail, Tone::Info);
            }
        }
    }

    fn start_recording(&mut self, register: char) {
        self.recording = Some(register);
        self.recorded.clear();
        self.say(format!("recording @{register} · q to stop"), Tone::Info);
    }

    fn stop_recording(&mut self) {
        let Some(register) = self.recording.take() else {
            return;
        };
        let keys = std::mem::take(&mut self.recorded);
        if keys.is_empty() {
            self.say(format!("@{register} is empty"), Tone::Info);
            return;
        }
        self.macros.insert(register, keys);
        self.last_macro = Some(register);
        self.say(format!("recorded @{register}"), Tone::Ok);
    }

    fn play(&mut self, register: Option<char>, times: u32, jobs: &mut Vec<Job>) {
        let Some(register) = register.or(self.last_macro) else {
            self.say("E30: no macro recorded", Tone::Err);
            return;
        };
        if register == ':' {
            match self.last_ex.clone() {
                Some(line) => self.run_line(&line, jobs),
                None => self.say("E30: no command to repeat", Tone::Err),
            }
            return;
        }
        let Some(keys) = self.macros.get(&register).cloned() else {
            self.say(format!("E30: no such register @{register}"), Tone::Err);
            return;
        };
        if self.replay_depth >= 4 {
            self.say("macro recursion is too deep", Tone::Err);
            return;
        }
        self.last_macro = Some(register);
        self.replay_depth += 1;
        'replay: for _ in 0..times.max(1) {
            for key in keys.iter().copied() {
                let jobs_from_key = self.key(key);
                jobs.extend(jobs_from_key);
                if self.quit {
                    break 'replay;
                }
            }
        }
        self.replay_depth -= 1;
        if self.replay_depth == 0 {
            self.change = Some(Change::Macro { register, times });
        }
    }

    fn history_move(&mut self, delta: i32) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let index = match self.history_at {
            None if delta < 0 => last as i32,
            None => return,
            Some(index) => (index as i32 + delta).clamp(0, last as i32),
        };
        self.history_at = Some(index as usize);
        if let Some(line) = self.history.get(index as usize) {
            self.line.set(line.clone());
        }
    }

    fn remember(&mut self, bundle: &str) {
        self.last_app = Some(bundle.to_string());
        self.recents.retain(|recent| recent != bundle);
        self.recents.insert(0, bundle.to_string());
        self.recents.truncate(RECENT_LIMIT);
    }

    fn app_name(&self, bundle: &str) -> String {
        self.apps
            .iter()
            .find(|(id, _)| id == bundle)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| bundle.to_string())
    }

    fn names(&self) -> Vec<String> {
        self.apps.iter().map(|(_, name)| name.clone()).collect()
    }

    fn say(&mut self, text: impl Into<String>, tone: Tone) {
        let text = text.into();
        self.message = Message {
            text: text.clone(),
            tone,
        };
        self.log(text, tone);
    }

    fn log(&mut self, text: impl Into<String>, tone: Tone) {
        let text = text.into();
        if let Some(recent) = self.activity.front_mut() {
            if recent.text == text && recent.tone == tone {
                recent.count += 1;
                return;
            }
        }
        self.activity.push_front(Activity {
            text,
            tone,
            count: 1,
        });
        self.activity.truncate(ACTIVITY_LIMIT);
    }

    fn apply(&mut self, outcome: Outcome) {
        if let Some(power) = outcome.power {
            self.power = power;
        }
        if let Some(volume) = outcome.volume {
            self.volume = volume;
        }
        if let Some(result) = outcome.apps {
            match result {
                Ok(apps) => {
                    self.apps = apps;
                    self.apps_loaded = true;
                    self.palette.loading = false;
                    self.palette.error = None;
                    self.palette.refilter(&self.apps, &self.recents, false);
                }
                Err(error) => {
                    self.palette.loading = false;
                    self.palette.error = Some(error.clone());
                    self.say(error, Tone::Err);
                }
            }
        }
        if outcome.silent {
            return;
        }
        self.say(outcome.message, outcome.tone);
    }

    fn device_event(&mut self, event: &Event) {
        if event.name != "SystemStatus" && event.name != "TVSystemStatus" {
            return;
        }
        if let Some(state) = event.content.get("state").and_then(|value| value.as_i64()) {
            self.power = PowerState::from_attention_state(state);
        }
    }
}

pub async fn run(client: Client, device: PairedDevice, timeout: Duration) -> Result<()> {
    let info = client.info.clone();
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, client, device, info, timeout).await;

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: Client,
    device: PairedDevice,
    info: crate::companion::client::DeviceInfo,
    timeout: Duration,
) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::channel(64);
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(event) => {
                if key_tx.blocking_send(event).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    });

    let (job_tx, job_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let mut device_events = client.events();
    tokio::spawn(worker(
        Session {
            client,
            device: device.clone(),
            timeout,
            retry_at: None,
        },
        job_rx,
        done_tx,
    ));

    let mut app = App::new(&device.name, info.os_version.clone(), info.model.clone());
    let mut events_open = true;
    let mut inflight = 0usize;
    let mut last_poll = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;

    loop {
        terminal.draw(|frame| view::draw(frame, &app))?;
        if app.quit {
            return Ok(());
        }

        tokio::select! {
            received = key_rx.recv() => {
                let Some(event) = received else { return Ok(()) };
                let TermEvent::Key(key) = event else { continue };
                let Some(key) = Key::from_event(key) else { continue };
                let jobs = app.key(key);
                inflight += jobs.len();
                let busy = !jobs.is_empty();
                for job in jobs {
                    let _ = job_tx.send(job);
                }
                if busy && app.power == PowerState::Off {
                    last_poll = tokio::time::Instant::now() - Duration::from_secs(20);
                }
            }
            outcome = done_rx.recv() => {
                let Some(outcome) = outcome else { continue };
                inflight = inflight.saturating_sub(1);
                app.apply(outcome);
            }
            event = device_events.recv(), if events_open => {
                match event {
                    Ok(event) => app.device_event(&event),
                    Err(_) => events_open = false,
                }
            }
            _ = tick.tick() => {
                let gap = if app.power == PowerState::Off {
                    Duration::from_secs(15)
                } else {
                    Duration::ZERO
                };
                if inflight == 0 && last_poll.elapsed() >= gap {
                    last_poll = tokio::time::Instant::now();
                    inflight += 1;
                    let _ = job_tx.send(Job::Poll);
                }
            }
        }
    }
}

struct Session {
    client: Client,
    device: PairedDevice,
    timeout: Duration,
    retry_at: Option<tokio::time::Instant>,
}

impl Session {
    async fn reconnect(&mut self) -> Result<()> {
        if let Some(retry_at) = self.retry_at {
            if tokio::time::Instant::now() < retry_at {
                return Err(Error::Protocol("the Apple TV is still offline".into()));
            }
        }
        match self.connect().await {
            Ok(()) => {
                self.retry_at = None;
                Ok(())
            }
            Err(error) => {
                self.retry_at = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                Err(error)
            }
        }
    }

    async fn connect(&mut self) -> Result<()> {
        let credentials = self.device.credentials()?;
        if let Ok(client) = Client::connect(
            &self.device.address,
            self.device.port,
            &credentials,
            self.timeout,
        )
        .await
        {
            self.client = client;
            return Ok(());
        }
        let found = discovery::scan(Duration::from_secs(2), false).await?;
        let Some(candidate) = found
            .iter()
            .find(|candidate| candidate.id == self.device.id)
        else {
            return Err(Error::Protocol(
                "the Apple TV did not answer discovery".into(),
            ));
        };
        if candidate.address != self.device.address || candidate.port != self.device.port {
            self.device.address = candidate.address.clone();
            self.device.port = candidate.port;
            if let Ok(mut store) = Store::load() {
                store.upsert(self.device.clone());
                let _ = store.save();
            }
        }
        let credentials = self.device.credentials()?;
        self.client = Client::connect(
            &self.device.address,
            self.device.port,
            &credentials,
            self.timeout,
        )
        .await?;
        Ok(())
    }
}

fn never_sent(error: &Error) -> bool {
    matches!(error, Error::Protocol(message)
        if message.contains("connection closed by the Apple TV")
            || message.contains("cannot queue frame"))
}

fn is_lost(error: &Error) -> bool {
    match error {
        Error::Io(_) | Error::Timeout(_) => true,
        Error::Protocol(message) => {
            message.contains("cannot queue frame") || message.contains("connection closed")
        }
        _ => false,
    }
}

async fn worker(
    mut session: Session,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    out: mpsc::UnboundedSender<Outcome>,
) {
    while let Some(job) = jobs.recv().await {
        let retry = job.clone();
        let mut outcome = execute(&session.client, job).await;
        if outcome.lost {
            crate::log::info(format_args!("connection lost: {}", outcome.message));
            let was_silent = outcome.silent;
            let unsent = outcome.unsent;
            match session.reconnect().await {
                Ok(()) => {
                    crate::log::info(format_args!("reconnected to {}", session.device.address));
                    if unsent {
                        outcome = execute(&session.client, retry).await;
                        outcome.lost = false;
                    } else {
                        if let Ok((power, volume)) = status(&session.client).await {
                            outcome.power = Some(power);
                            outcome.volume = Some(volume);
                        }
                        if !was_silent {
                            outcome.tone = Tone::Info;
                            outcome.message =
                                format!("{} · reconnected, try again", outcome.message);
                        }
                    }
                }
                Err(error) => {
                    if !was_silent {
                        outcome.message = format!("{} · {error}", outcome.message);
                    }
                }
            }
            outcome.lost = false;
        }
        if out.send(outcome).is_err() {
            return;
        }
    }
}

async fn execute(client: &Client, job: Job) -> Outcome {
    match job {
        Job::Press {
            command,
            times,
            action,
        } => {
            for _ in 0..times {
                if let Err(error) = client.press(command, action, HOLD).await {
                    return Outcome::err(error);
                }
            }
            let label = match action {
                InputAction::Hold => format!("held {}", command.name()),
                InputAction::Double => format!("double {}", command.name()),
                InputAction::Single => command.name().to_string(),
            };
            Outcome::ok(if times == 1 {
                label
            } else {
                format!("{label} ×{times}")
            })
        }
        Job::Volume { up, times } => {
            let command = if up {
                HidCommand::VolumeUp
            } else {
                HidCommand::VolumeDown
            };
            for _ in 0..times.max(1) {
                if let Err(error) = client
                    .press(command, InputAction::Single, Duration::ZERO)
                    .await
                {
                    return Outcome::err(error);
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            let level = client.volume().await.ok().flatten();
            let message = match level {
                Some(level) => format!("volume {level:.0}%"),
                None => format!("volume {}", if up { "up" } else { "down" }),
            };
            let mut outcome = Outcome::ok(message);
            outcome.volume = Some(level);
            outcome
        }
        Job::SetVolume(percent) => {
            if let Err(error) = client.set_volume(percent).await {
                return Outcome::err(error);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            let level = client.volume().await.ok().flatten();
            let mut outcome = Outcome::ok(match level {
                Some(level) => format!("volume {level:.0}%"),
                None => format!("volume set to {percent:.0}%"),
            });
            outcome.volume = Some(level);
            outcome
        }
        Job::Seek(seconds) => match client.skip(seconds).await {
            Ok(()) => Outcome::ok(format!("skipped {seconds:+.0}s")),
            Err(error) => Outcome::err(error),
        },
        Job::TogglePower => {
            let state = client.power_state().await;
            let sleeping = state == PowerState::On;
            let command = if sleeping {
                HidCommand::Sleep
            } else {
                HidCommand::Wake
            };
            if let Err(error) = client.press(command, InputAction::Single, HOLD).await {
                return Outcome::err(error);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let power = client.power_state().await;
            let mut outcome = Outcome::ok(if sleeping {
                "going to sleep".to_string()
            } else {
                "waking the Apple TV".to_string()
            });
            outcome.power = Some(power);
            outcome
        }
        Job::Sleep => power_press(client, HidCommand::Sleep, "going to sleep").await,
        Job::Wake => power_press(client, HidCommand::Wake, "waking the Apple TV").await,
        Job::Siri => match client
            .press(
                HidCommand::Siri,
                InputAction::Single,
                Duration::from_millis(600),
            )
            .await
        {
            Ok(()) => Outcome::ok("Siri button held"),
            Err(error) => Outcome::err(error),
        },
        Job::Click { double } => {
            let action = if double {
                InputAction::Double
            } else {
                InputAction::Single
            };
            match client.click(action).await {
                Ok(()) => Outcome::ok(if double { "double click" } else { "click" }),
                Err(error) => Outcome::err(error),
            }
        }
        Job::Swipe(direction) => match client.swipe(direction.gesture()).await {
            Ok(()) => Outcome::ok(format!("swiped {}", direction.name())),
            Err(error) => Outcome::err(error),
        },
        Job::Launch { bundle, name } => match client.launch_app(&bundle).await {
            Ok(()) => Outcome::ok(format!("launched {name}")),
            Err(error) => Outcome::err(error),
        },
        Job::OpenUrl(url) => match client.launch_app(&url).await {
            Ok(()) => Outcome::ok(format!("opened {url}")),
            Err(error) => Outcome::err(error),
        },
        Job::Account(query) => {
            let accounts = match client.account_list().await {
                Ok(accounts) => accounts,
                Err(error) => return Outcome::err(error),
            };
            if accounts.is_empty() {
                return Outcome::note("this Apple TV reports no user accounts", Tone::Err);
            }
            let best = accounts
                .iter()
                .filter_map(|(id, name)| {
                    fuzzy::score(&query, name).map(|hit| (id, name, hit.score))
                })
                .max_by_key(|(_, _, score)| *score);
            match best {
                Some((id, name, _)) => match client.switch_account(id).await {
                    Ok(()) => Outcome::ok(format!("switched to {name}")),
                    Err(error) => Outcome::err(error),
                },
                None => Outcome::note(format!("no account matches \"{query}\""), Tone::Err),
            }
        }
        Job::Text { value, append } => {
            let characters = value.chars().count();
            let sent = if append {
                client.text_append(&value).await
            } else {
                client.text_set(&value).await
            };
            match sent {
                Ok(_) => Outcome::ok(format!(
                    "{} {characters} character{}",
                    if append { "appended" } else { "typed" },
                    if characters == 1 { "" } else { "s" }
                )),
                Err(error) => Outcome::err(error),
            }
        }
        Job::LoadApps => {
            let mut outcome = Outcome::quiet();
            outcome.apps = Some(client.app_list().await.map_err(|error| error.to_string()));
            outcome
        }
        Job::Refresh => match status(client).await {
            Ok((power, volume)) => {
                let mut outcome = Outcome::ok("refreshed");
                outcome.power = Some(power);
                outcome.volume = Some(volume);
                outcome
            }
            Err(error) => Outcome::err(error),
        },
        Job::Poll => match status(client).await {
            Ok((power, volume)) => {
                let mut outcome = Outcome::quiet();
                outcome.power = Some(power);
                outcome.volume = Some(volume);
                outcome
            }
            Err(error) => {
                let mut outcome = Outcome::err(error);
                outcome.silent = true;
                outcome
            }
        },
    }
}

async fn status(client: &Client) -> Result<(PowerState, Option<f64>)> {
    let volume = client.volume().await?;
    Ok((client.power_state().await, volume))
}

async fn power_press(client: &Client, command: HidCommand, message: &str) -> Outcome {
    if let Err(error) = client.press(command, InputAction::Single, HOLD).await {
        return Outcome::err(error);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut outcome = Outcome::ok(message.to_string());
    outcome.power = Some(client.power_state().await);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_editing_moves_the_cursor_and_kills_words() {
        let mut line = Line::default();
        line.set("hello world".into());
        line.home();
        line.insert('>');
        assert_eq!(line.text(), ">hello world");
        line.end();
        line.kill_word();
        assert_eq!(line.text(), ">hello ");
        line.kill_to_start();
        assert_eq!(line.text(), "");
        assert_eq!(line.cursor, 0);
    }

    #[test]
    fn palette_ranks_prefixes_and_recents() {
        let apps = vec![
            ("com.apple.tv".to_string(), "Apple TV".to_string()),
            ("com.netflix".to_string(), "Netflix".to_string()),
            ("com.notes".to_string(), "Notes".to_string()),
        ];
        let mut palette = Palette::default();
        palette.open("net".into());
        palette.refilter(&apps, &[], true);
        assert_eq!(palette.list[0].name, "Netflix");

        palette.open(String::new());
        palette.refilter(&apps, &["com.notes".to_string()], true);
        assert_eq!(palette.list[0].name, "Notes");
    }

    #[test]
    fn app_keys_drive_the_parser_and_record_macros() {
        let mut app = App::new("Apple TV", Some("18.6".into()), None);
        assert!(app.key(Key::Char('q')).is_empty());
        app.key(Key::Char('a'));
        app.key(Key::Char('h'));
        app.key(Key::Char('q'));
        assert!(app.macros.contains_key(&'a'));

        let jobs = app.key(Key::Char('@'));
        assert!(jobs.is_empty());
        let _ = app.key(Key::Char('a'));
    }

    #[test]
    fn dot_repeats_the_last_macro() {
        let mut app = App::new("Apple TV", None, None);
        app.key(Key::Char('q'));
        app.key(Key::Char('a'));
        app.key(Key::Char('j'));
        app.key(Key::Char('q'));
        app.key(Key::Char('@'));
        assert_eq!(app.key(Key::Char('a')).len(), 1);
        assert_eq!(app.key(Key::Char('.')).len(), 1);
    }

    #[test]
    fn dot_repeats_the_last_motion() {
        let mut app = App::new("Apple TV", None, None);
        app.key(Key::Char('2'));
        app.key(Key::Char('j'));
        let jobs = app.key(Key::Char('.'));
        assert_eq!(jobs.len(), 1);
    }
}
