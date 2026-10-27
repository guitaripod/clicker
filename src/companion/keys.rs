//! HID and media-control command vocabularies.

use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Single,
    Double,
    Hold,
}

impl InputAction {
    pub fn parse(value: &str) -> Option<InputAction> {
        match value {
            "single" | "tap" | "click" => Some(InputAction::Single),
            "double" | "double-tap" | "doubletap" => Some(InputAction::Double),
            "hold" | "long" => Some(InputAction::Hold),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidCommand {
    Up = 1,
    Down = 2,
    Left = 3,
    Right = 4,
    Menu = 5,
    Select = 6,
    Home = 7,
    VolumeUp = 8,
    VolumeDown = 9,
    Siri = 10,
    Screensaver = 11,
    Sleep = 12,
    Wake = 13,
    PlayPause = 14,
    ChannelUp = 15,
    ChannelDown = 16,
    Guide = 17,
    PageUp = 18,
    PageDown = 19,
}

impl HidCommand {
    pub fn parse(value: &str) -> Option<HidCommand> {
        let normalized = value.to_ascii_lowercase().replace('_', "-");
        Some(match normalized.as_str() {
            "up" => HidCommand::Up,
            "down" => HidCommand::Down,
            "left" => HidCommand::Left,
            "right" => HidCommand::Right,
            "menu" | "back" => HidCommand::Menu,
            "select" | "enter" | "ok" => HidCommand::Select,
            "home" | "tv" => HidCommand::Home,
            "volume-up" | "vol-up" | "louder" => HidCommand::VolumeUp,
            "volume-down" | "vol-down" | "quieter" => HidCommand::VolumeDown,
            "siri" => HidCommand::Siri,
            "screensaver" => HidCommand::Screensaver,
            "sleep" => HidCommand::Sleep,
            "wake" => HidCommand::Wake,
            "play-pause" | "playpause" | "pause" => HidCommand::PlayPause,
            "channel-up" | "channel-next" => HidCommand::ChannelUp,
            "channel-down" | "channel-previous" => HidCommand::ChannelDown,
            "guide" => HidCommand::Guide,
            "page-up" => HidCommand::PageUp,
            "page-down" | "control-center" => HidCommand::PageDown,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            HidCommand::Up => "up",
            HidCommand::Down => "down",
            HidCommand::Left => "left",
            HidCommand::Right => "right",
            HidCommand::Menu => "menu",
            HidCommand::Select => "select",
            HidCommand::Home => "home",
            HidCommand::VolumeUp => "volume-up",
            HidCommand::VolumeDown => "volume-down",
            HidCommand::Siri => "siri",
            HidCommand::Screensaver => "screensaver",
            HidCommand::Sleep => "sleep",
            HidCommand::Wake => "wake",
            HidCommand::PlayPause => "play-pause",
            HidCommand::ChannelUp => "channel-up",
            HidCommand::ChannelDown => "channel-down",
            HidCommand::Guide => "guide",
            HidCommand::PageUp => "page-up",
            HidCommand::PageDown => "page-down",
        }
    }

    pub const ALL: [HidCommand; 19] = [
        HidCommand::Up,
        HidCommand::Down,
        HidCommand::Left,
        HidCommand::Right,
        HidCommand::Menu,
        HidCommand::Select,
        HidCommand::Home,
        HidCommand::VolumeUp,
        HidCommand::VolumeDown,
        HidCommand::Siri,
        HidCommand::Screensaver,
        HidCommand::Sleep,
        HidCommand::Wake,
        HidCommand::PlayPause,
        HidCommand::ChannelUp,
        HidCommand::ChannelDown,
        HidCommand::Guide,
        HidCommand::PageUp,
        HidCommand::PageDown,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    Play = 1,
    Pause = 2,
    NextTrack = 3,
    PreviousTrack = 4,
    GetVolume = 5,
    SetVolume = 6,
    SkipBy = 7,
    FastForwardBegin = 8,
    FastForwardEnd = 9,
    RewindBegin = 10,
    RewindEnd = 11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Press = 1,
    Hold = 3,
    Release = 4,
    Click = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    On,
    Off,
    Unknown,
}

impl PowerState {
    pub fn as_str(self) -> &'static str {
        match self {
            PowerState::On => "on",
            PowerState::Off => "off",
            PowerState::Unknown => "unknown",
        }
    }

    /// System status codes from `FetchAttentionState`.
    pub fn from_attention_state(state: i64) -> PowerState {
        match state {
            0x01 => PowerState::Off,
            0x02..=0x04 => PowerState::On,
            _ => PowerState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Gesture {
    pub from: (i64, i64),
    pub to: (i64, i64),
    pub duration: Duration,
}

impl fmt::Display for Gesture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, {}) -> ({}, {}) in {}ms",
            self.from.0,
            self.from.1,
            self.to.0,
            self.to.1,
            self.duration.as_millis()
        )
    }
}
