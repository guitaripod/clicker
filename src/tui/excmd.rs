//! The `:` command line: names, arguments and completion.

use crate::companion::keys::HidCommand;

use super::SwipeDir;

#[derive(Debug, Clone, PartialEq)]
pub enum Ex {
    Noop,
    Quit,
    Power(Option<bool>),
    Sleep,
    Wake,
    VolumeShow,
    VolumeSet(f64),
    VolumeStep(i32),
    Seek(f64),
    App(Option<String>),
    Apps,
    Open(String),
    Account(String),
    Siri,
    Click(bool),
    Press(HidCommand),
    Page(bool),
    Swipe(SwipeDir),
    Text(Option<String>),
    Refresh,
    Help,
    Version,
    Device,
}

pub const COMMANDS: &[&str] = &[
    "account",
    "app",
    "apps",
    "back",
    "click",
    "device",
    "double",
    "guide",
    "help",
    "home",
    "open",
    "page",
    "play",
    "power",
    "quit",
    "refresh",
    "screensaver",
    "seek",
    "select",
    "siri",
    "sleep",
    "swipe",
    "text",
    "version",
    "volume",
    "wake",
];

pub fn parse(line: &str) -> Result<Ex, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Ex::Noop);
    }
    let mut pieces = line.splitn(2, char::is_whitespace);
    let mut name = pieces.next().unwrap_or_default().to_ascii_lowercase();
    let argument = pieces.next().unwrap_or_default().trim();
    if name.ends_with('!') {
        name.pop();
    }

    if matches!(
        name.as_str(),
        "q" | "qu" | "qui" | "quit" | "qa" | "qall" | "x" | "wq"
    ) {
        return Ok(Ex::Quit);
    }

    Ok(match name.as_str() {
        "pow" | "power" => Ex::Power(power_argument(argument)?),
        "sl" | "sleep" => Ex::Sleep,
        "wa" | "wake" => Ex::Wake,
        "vol" | "volume" => volume_argument(argument)?,
        "seek" | "sk" => Ex::Seek(number(argument, "seek")?),
        "app" => Ex::App(optional(argument)),
        "apps" | "launcher" => Ex::Apps,
        "open" | "url" => Ex::Open(required(argument, "open")?),
        "account" | "acc" | "user" => Ex::Account(required(argument, "account")?),
        "siri" => Ex::Siri,
        "click" | "tap" => Ex::Click(false),
        "double" | "dblclick" => Ex::Click(true),
        "home" | "tv" => Ex::Press(HidCommand::Home),
        "back" | "menu" => Ex::Press(HidCommand::Menu),
        "select" | "enter" | "ok" | "return" => Ex::Press(HidCommand::Select),
        "play" | "pause" | "playpause" | "play-pause" => Ex::Press(HidCommand::PlayPause),
        "guide" => Ex::Press(HidCommand::Guide),
        "screensaver" | "saver" => Ex::Press(HidCommand::Screensaver),
        "page" => Ex::Page(!matches!(argument, "up")),
        "swipe" => Ex::Swipe(swipe_argument(argument)?),
        "text" | "type" => Ex::Text(optional(argument)),
        "refresh" | "reload" | "r" => Ex::Refresh,
        "help" | "keys" | "keymap" | "h" => Ex::Help,
        "version" | "ver" => Ex::Version,
        "device" | "info" => Ex::Device,
        other => return Err(unknown(other)),
    })
}

fn unknown(name: &str) -> String {
    match suggest(name) {
        Some(hint) => format!("E492: Not an editor command: {name} (did you mean :{hint}?)"),
        None => format!("E492: Not an editor command: {name}"),
    }
}

fn suggest(name: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .find(|command| command.starts_with(name) || name.starts_with(*command))
}

fn optional(argument: &str) -> Option<String> {
    (!argument.is_empty()).then(|| argument.to_string())
}

fn required(argument: &str, name: &str) -> Result<String, String> {
    if argument.is_empty() {
        Err(format!("E471: Argument required: {name}"))
    } else {
        Ok(argument.to_string())
    }
}

fn number(argument: &str, name: &str) -> Result<f64, String> {
    argument
        .parse::<f64>()
        .map_err(|_| format!("E474: Invalid argument for {name}: {argument}"))
}

fn power_argument(argument: &str) -> Result<Option<bool>, String> {
    Ok(match argument {
        "" | "toggle" => None,
        "on" | "1" | "true" | "wake" => Some(true),
        "off" | "0" | "false" | "sleep" => Some(false),
        other => return Err(format!("E474: Invalid argument for power: {other}")),
    })
}

fn volume_argument(argument: &str) -> Result<Ex, String> {
    if argument.is_empty() {
        return Ok(Ex::VolumeShow);
    }
    if let Some(steps) = argument.strip_prefix('+') {
        return Ok(Ex::VolumeStep(number(steps, "volume")?.max(0.0) as i32));
    }
    if let Some(steps) = argument.strip_prefix('-') {
        return Ok(Ex::VolumeStep(-(number(steps, "volume")?.max(0.0) as i32)));
    }
    let level = number(argument, "volume")?;
    if !(0.0..=100.0).contains(&level) {
        return Err(format!("E474: Volume must be between 0 and 100: {level}"));
    }
    Ok(Ex::VolumeSet(level))
}

fn swipe_argument(argument: &str) -> Result<SwipeDir, String> {
    Ok(match argument {
        "left" | "l" => SwipeDir::Left,
        "right" | "r" => SwipeDir::Right,
        "up" | "u" => SwipeDir::Up,
        "down" | "d" => SwipeDir::Down,
        other => return Err(format!("E474: Invalid argument for swipe: {other}")),
    })
}

/// The line completion would produce, or `None` when nothing (or more than
/// one thing) matches.
pub fn complete(input: &str, names: &[String]) -> Option<String> {
    match input.split_once(char::is_whitespace) {
        None => {
            let token = input.to_ascii_lowercase();
            let mut matches = COMMANDS
                .iter()
                .copied()
                .filter(|command| command.starts_with(&token));
            let first = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            Some(first.to_string())
        }
        Some((head, rest)) => {
            let candidates = arguments(&head.to_ascii_lowercase(), names)?;
            let prefix = rest.trim_start().to_ascii_lowercase();
            let mut matches = candidates
                .into_iter()
                .filter(|candidate| candidate.to_ascii_lowercase().starts_with(&prefix));
            let first = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            Some(format!("{} {}", head, first))
        }
    }
}

fn arguments(command: &str, names: &[String]) -> Option<Vec<String>> {
    let static_words: &[&str] = match command {
        "power" | "pow" => &["on", "off", "toggle"],
        "page" => &["up", "down"],
        "swipe" => &["up", "down", "left", "right"],
        "app" => return Some(names.to_vec()),
        _ => return None,
    };
    Some(static_words.iter().map(|word| word.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::{complete, parse, Ex};
    use crate::companion::keys::HidCommand;

    #[test]
    fn quitting_takes_its_vim_forms() {
        for line in ["q", "q!", "quit", "wq", "x", "qa"] {
            assert_eq!(parse(line), Ok(Ex::Quit), "{line}");
        }
    }

    #[test]
    fn power_takes_on_off_and_toggle() {
        assert_eq!(parse("power on"), Ok(Ex::Power(Some(true))));
        assert_eq!(parse("power off"), Ok(Ex::Power(Some(false))));
        assert_eq!(parse("power"), Ok(Ex::Power(None)));
        assert!(parse("power sideways").is_err());
    }

    #[test]
    fn volume_understands_steps_and_levels() {
        assert_eq!(parse("vol 40"), Ok(Ex::VolumeSet(40.0)));
        assert_eq!(parse("volume +5"), Ok(Ex::VolumeStep(5)));
        assert_eq!(parse("volume -3"), Ok(Ex::VolumeStep(-3)));
        assert_eq!(parse("vol"), Ok(Ex::VolumeShow));
        assert!(parse("vol 400").is_err());
    }

    #[test]
    fn text_and_apps_keep_their_arguments() {
        assert_eq!(
            parse("text blade runner"),
            Ok(Ex::Text(Some("blade runner".into())))
        );
        assert_eq!(parse("text"), Ok(Ex::Text(None)));
        assert_eq!(parse("app net"), Ok(Ex::App(Some("net".into()))));
        assert_eq!(parse("apps"), Ok(Ex::Apps));
        assert_eq!(
            parse("open https://example.com"),
            Ok(Ex::Open("https://example.com".into()))
        );
    }

    #[test]
    fn remote_buttons_map_to_presses() {
        assert_eq!(parse("home"), Ok(Ex::Press(HidCommand::Home)));
        assert_eq!(parse("back"), Ok(Ex::Press(HidCommand::Menu)));
        assert_eq!(parse("select"), Ok(Ex::Press(HidCommand::Select)));
        assert_eq!(parse("click"), Ok(Ex::Click(false)));
        assert_eq!(parse("double"), Ok(Ex::Click(true)));
        assert_eq!(parse("page up"), Ok(Ex::Page(false)));
        assert_eq!(parse("page down"), Ok(Ex::Page(true)));
    }

    #[test]
    fn unknown_commands_carry_a_vim_error_and_a_hint() {
        let error = parse("ap").unwrap_err();
        assert!(error.starts_with("E492:"), "{error}");
        assert!(error.contains("did you mean :app?"), "{error}");
        let error = parse("frobnicate").unwrap_err();
        assert!(error.starts_with("E492:"), "{error}");
    }

    #[test]
    fn required_arguments_are_reported() {
        assert!(parse("open").unwrap_err().starts_with("E471:"));
        assert!(parse("account").unwrap_err().starts_with("E471:"));
        assert!(parse("swipe sideways").unwrap_err().starts_with("E474:"));
    }

    #[test]
    fn completion_finishes_unique_commands_and_arguments() {
        assert_eq!(complete("pow", &[]), Some("power".into()));
        assert_eq!(complete("see", &[]), Some("seek".into()));
        assert_eq!(complete("", &[]), None);
        assert_eq!(complete("power of", &[]), Some("power off".into()));
        assert_eq!(complete("swipe l", &[]), Some("swipe left".into()));
        let names = vec!["Netflix".to_string(), "Notes".to_string()];
        assert_eq!(complete("app net", &names), Some("app Netflix".into()));
        assert_eq!(complete("app n", &names), None);
    }
}
