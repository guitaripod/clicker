//! Vim key grammar: counts, prefixes and the remote actions they produce.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::companion::keys::HidCommand;

use super::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    pub fn from_event(event: KeyEvent) -> Option<Key> {
        if event.kind == KeyEventKind::Release {
            return None;
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char(c) = event.code {
                return Some(Key::Ctrl(c.to_ascii_lowercase()));
            }
        }
        Some(match event.code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Press(HidCommand),
    Hold(HidCommand),
    Volume(i32),
    Seek(f64),
    TogglePower,
    Siri,
    Click { double: bool },
    Refresh,
    LastApp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Nothing,
    Pending,
    Run(Action, u32),
    Open(Mode),
    Record(Option<char>),
    Play(Option<char>, u32),
    Repeat(Option<u32>),
    Quit,
}

#[derive(Debug, Default, Clone)]
pub struct Parser {
    count: Option<u32>,
    prefix: Option<char>,
}

impl Parser {
    pub fn idle(&self) -> bool {
        self.count.is_none() && self.prefix.is_none()
    }

    pub fn pending(&self) -> String {
        let mut label = String::new();
        if let Some(count) = self.count {
            label.push_str(&count.to_string());
        }
        if let Some(prefix) = self.prefix {
            label.push(prefix);
        }
        label
    }

    pub fn clear(&mut self) {
        self.count = None;
        self.prefix = None;
    }

    pub fn feed(&mut self, key: Key, recording: bool) -> Parsed {
        if let Some(prefix) = self.prefix.take() {
            return self.after_prefix(prefix, key);
        }
        if recording && key == Key::Char('q') && self.count.is_none() {
            self.clear();
            return Parsed::Record(None);
        }
        match key {
            Key::Esc => {
                self.clear();
                Parsed::Nothing
            }
            Key::Char(c @ '1'..='9') => {
                self.push_digit(c);
                Parsed::Pending
            }
            Key::Char('0') if self.count.is_some() => {
                self.push_digit('0');
                Parsed::Pending
            }
            other => self.command(other),
        }
    }

    fn after_prefix(&mut self, prefix: char, key: Key) -> Parsed {
        match (prefix, key) {
            ('g', Key::Char('g')) => self.run(Action::Press(HidCommand::Home)),
            ('q', Key::Char(c)) if c.is_ascii_lowercase() => Parsed::Record(Some(c)),
            ('@', Key::Char('@')) => Parsed::Play(None, self.take_count()),
            ('@', Key::Char(':')) => Parsed::Play(Some(':'), self.take_count()),
            ('@', Key::Char(c)) if c.is_ascii_lowercase() => {
                Parsed::Play(Some(c), self.take_count())
            }
            ('Z', Key::Char('Z')) | ('Z', Key::Char('Q')) => Parsed::Quit,
            _ => {
                self.clear();
                Parsed::Nothing
            }
        }
    }

    fn command(&mut self, key: Key) -> Parsed {
        match key {
            Key::Char('h') | Key::Left => self.run(Action::Press(HidCommand::Left)),
            Key::Char('j') | Key::Down => self.run(Action::Press(HidCommand::Down)),
            Key::Char('k') | Key::Up => self.run(Action::Press(HidCommand::Up)),
            Key::Char('l') | Key::Right => self.run(Action::Press(HidCommand::Right)),
            Key::Char('H') | Key::Home => self.run(Action::Press(HidCommand::Home)),
            Key::Char('K') => self.run(Action::Hold(HidCommand::Up)),
            Key::Char('J') => self.run(Action::Hold(HidCommand::Down)),
            Key::Enter | Key::Char(' ') => self.run(Action::Press(HidCommand::Select)),
            Key::Char('m') => self.run(Action::Press(HidCommand::Menu)),
            Key::Char('P') => self.run(Action::Press(HidCommand::PlayPause)),
            Key::Char('p') => self.run(Action::TogglePower),
            Key::Char('s') => self.run(Action::Siri),
            Key::Char('S') => self.run(Action::Press(HidCommand::Screensaver)),
            Key::Char('c') => self.run(Action::Click {
                double: self.count.unwrap_or(1) > 1,
            }),
            Key::Char('w') => self.run(Action::Seek(10.0)),
            Key::Char('b') => self.run(Action::Seek(-10.0)),
            Key::Char('[') => self.run(Action::Press(HidCommand::ChannelDown)),
            Key::Char(']') => self.run(Action::Press(HidCommand::ChannelUp)),
            Key::Char('>') | Key::Ctrl('a') => self.run(Action::Volume(1)),
            Key::Char('<') | Key::Ctrl('x') => self.run(Action::Volume(-1)),
            Key::Char('r') | Key::Ctrl('l') => self.run(Action::Refresh),
            Key::Char('G') => self.run(Action::LastApp),
            Key::Char('i') | Key::Char('t') => self.open(Mode::Insert { append: false }),
            Key::Char('a') => self.open(Mode::Insert { append: true }),
            Key::Char('o') | Key::Char('/') => self.open(Mode::Apps),
            Key::Char(':') => self.open(Mode::Command),
            Key::Char('?') => self.open(Mode::Help),
            Key::Char('.') => {
                let count = self.count.take();
                Parsed::Repeat(count)
            }
            Key::Ctrl('d') | Key::Ctrl('f') | Key::PageDown => {
                self.run(Action::Press(HidCommand::PageDown))
            }
            Key::Ctrl('u') | Key::Ctrl('b') | Key::PageUp => {
                self.run(Action::Press(HidCommand::PageUp))
            }
            Key::Char('g') | Key::Char('q') | Key::Char('@') | Key::Char('Z') => {
                let Key::Char(prefix) = key else {
                    unreachable!()
                };
                self.prefix = Some(prefix);
                Parsed::Pending
            }
            _ => {
                self.clear();
                Parsed::Nothing
            }
        }
    }

    fn open(&mut self, mode: Mode) -> Parsed {
        self.clear();
        Parsed::Open(mode)
    }

    fn run(&mut self, action: Action) -> Parsed {
        Parsed::Run(action, self.take_count())
    }

    fn take_count(&mut self) -> u32 {
        self.count.take().unwrap_or(1)
    }

    fn push_digit(&mut self, digit: char) {
        let value = digit.to_digit(10).unwrap_or(0);
        let count = self.count.unwrap_or(0).saturating_mul(10) + value;
        self.count = Some(count.clamp(1, 999));
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Key, Parsed, Parser};
    use crate::companion::keys::HidCommand;
    use crate::tui::Mode;

    fn feed(keys: &[Key]) -> (Parser, Parsed) {
        let mut parser = Parser::default();
        let mut last = Parsed::Nothing;
        for key in keys {
            last = parser.feed(*key, false);
        }
        (parser, last)
    }

    #[test]
    fn counts_prefix_the_next_motion() {
        let (parser, parsed) = feed(&[Key::Char('3'), Key::Char('j')]);
        assert_eq!(parsed, Parsed::Run(Action::Press(HidCommand::Down), 3));
        assert!(parser.idle());
    }

    #[test]
    fn counts_can_be_long_and_zero_is_ignored_alone() {
        let (_, parsed) = feed(&[Key::Char('1'), Key::Char('2'), Key::Char('l')]);
        assert_eq!(parsed, Parsed::Run(Action::Press(HidCommand::Right), 12));
        let (parser, parsed) = feed(&[Key::Char('0')]);
        assert_eq!(parsed, Parsed::Nothing);
        assert_eq!(parser.pending(), "");
    }

    #[test]
    fn gg_goes_home_and_lone_g_waits() {
        let (parser, parsed) = feed(&[Key::Char('g')]);
        assert_eq!(parsed, Parsed::Pending);
        assert_eq!(parser.pending(), "g");
        let (_, parsed) = feed(&[Key::Char('g'), Key::Char('g')]);
        assert_eq!(parsed, Parsed::Run(Action::Press(HidCommand::Home), 1));
    }

    #[test]
    fn macros_record_start_and_stop() {
        let (_, parsed) = feed(&[Key::Char('q'), Key::Char('a')]);
        assert_eq!(parsed, Parsed::Record(Some('a')));
        let mut parser = Parser::default();
        assert_eq!(parser.feed(Key::Char('q'), true), Parsed::Record(None));
        assert_eq!(parser.feed(Key::Char('q'), false), Parsed::Pending);
    }

    #[test]
    fn macros_replay_with_counts() {
        let (_, parsed) = feed(&[Key::Char('3'), Key::Char('@'), Key::Char('a')]);
        assert_eq!(parsed, Parsed::Play(Some('a'), 3));
        let (_, parsed) = feed(&[Key::Char('@'), Key::Char('@')]);
        assert_eq!(parsed, Parsed::Play(None, 1));
        let (_, parsed) = feed(&[Key::Char('@'), Key::Char(':')]);
        assert_eq!(parsed, Parsed::Play(Some(':'), 1));
    }

    #[test]
    fn volume_uses_vim_increment_and_angle_brackets() {
        let (_, parsed) = feed(&[Key::Char('>')]);
        assert_eq!(parsed, Parsed::Run(Action::Volume(1), 1));
        let (_, parsed) = feed(&[Key::Char('<')]);
        assert_eq!(parsed, Parsed::Run(Action::Volume(-1), 1));
        let (_, parsed) = feed(&[Key::Ctrl('a')]);
        assert_eq!(parsed, Parsed::Run(Action::Volume(1), 1));
        let (_, parsed) = feed(&[Key::Ctrl('x')]);
        assert_eq!(parsed, Parsed::Run(Action::Volume(-1), 1));
        let (_, parsed) = feed(&[Key::Char('3'), Key::Char('>')]);
        assert_eq!(parsed, Parsed::Run(Action::Volume(1), 3));
    }

    #[test]
    fn plus_and_minus_are_left_to_the_terminal() {
        let (parser, parsed) = feed(&[Key::Char('+')]);
        assert_eq!(parsed, Parsed::Nothing);
        assert!(parser.idle());
        let (_, parsed) = feed(&[Key::Char('-')]);
        assert_eq!(parsed, Parsed::Nothing);
        let (_, parsed) = feed(&[Key::Char('=')]);
        assert_eq!(parsed, Parsed::Nothing);
        let (_, parsed) = feed(&[Key::Char('_')]);
        assert_eq!(parsed, Parsed::Nothing);
    }

    #[test]
    fn modes_open_on_their_letters() {
        let (_, parsed) = feed(&[Key::Char(':')]);
        assert_eq!(parsed, Parsed::Open(Mode::Command));
        let (_, parsed) = feed(&[Key::Char('i')]);
        assert_eq!(parsed, Parsed::Open(Mode::Insert { append: false }));
        let (_, parsed) = feed(&[Key::Char('a')]);
        assert_eq!(parsed, Parsed::Open(Mode::Insert { append: true }));
        let (_, parsed) = feed(&[Key::Char('o')]);
        assert_eq!(parsed, Parsed::Open(Mode::Apps));
        let (_, parsed) = feed(&[Key::Char('?')]);
        assert_eq!(parsed, Parsed::Open(Mode::Help));
    }

    #[test]
    fn dot_repeats_and_carries_its_counts() {
        let (_, parsed) = feed(&[Key::Char('3'), Key::Char('.')]);
        assert_eq!(parsed, Parsed::Repeat(Some(3)));
        let (_, parsed) = feed(&[Key::Char('.')]);
        assert_eq!(parsed, Parsed::Repeat(None));
    }

    #[test]
    fn escape_clears_pending_keys() {
        let (parser, _) = feed(&[Key::Char('9'), Key::Char('g'), Key::Esc]);
        assert!(parser.idle());
    }

    #[test]
    fn unknown_keys_cancel_the_count() {
        let (parser, parsed) = feed(&[Key::Char('5'), Key::Char('v')]);
        assert_eq!(parsed, Parsed::Nothing);
        assert!(parser.idle());
    }

    #[test]
    fn uppercase_z_quits() {
        let (_, parsed) = feed(&[Key::Char('Z'), Key::Char('Q')]);
        assert_eq!(parsed, Parsed::Quit);
        let (_, parsed) = feed(&[Key::Char('Z'), Key::Char('Z')]);
        assert_eq!(parsed, Parsed::Quit);
    }
}
