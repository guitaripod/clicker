//! Output discipline: human text on a terminal, one JSON object per command
//! with `ok` at the top level, and errors that never pollute stdout in text mode.

use std::io::IsTerminal;

use serde_json::{json, Value};

use crate::Error;

const RESET: &str = "\x1b[0m";

pub struct Output {
    pub json: bool,
    color: bool,
}

impl Output {
    pub fn new(json: bool) -> Self {
        let color = !json
            && std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM")
                .map(|term| term != "dumb")
                .unwrap_or(true);
        Self { json, color }
    }

    pub fn ok(&self, payload: Value) {
        if !self.json {
            return;
        }
        match payload {
            Value::Object(mut map) => {
                map.insert("ok".into(), Value::Bool(true));
                println!("{}", Value::Object(map));
            }
            other => println!("{}", json!({ "ok": true, "data": other })),
        }
    }

    /// Emit a human line; JSON mode leaves rendering to [`Output::ok`].
    pub fn text(&self, text: impl AsRef<str>) {
        if !self.json {
            println!("{}", text.as_ref());
        }
    }

    pub fn warn(&self, text: impl AsRef<str>) {
        eprintln!("clicker: {}", text.as_ref());
    }

    /// Progress or recovery information: stderr, so it never corrupts JSON.
    pub fn note(&self, text: std::fmt::Arguments<'_>) {
        eprintln!("{} {}", self.dim("clicker:"), text);
    }

    /// One JSON object per line, used by streaming commands. Text mode renders
    /// the same event as a readable line.
    pub fn stream(&self, event: Value) {
        if self.json {
            println!("{}", json!({ "ok": true, "event": event }));
        } else {
            println!("{}", render_event(&event));
        }
    }

    /// A machine-only progress event: silent on a terminal.
    pub fn event(&self, event: Value) {
        if self.json {
            println!("{}", json!({ "ok": true, "event": event }));
        }
    }

    pub fn error(&self, error: &Error) {
        if self.json {
            let mut payload = json!({
                "ok": false,
                "error": {
                    "code": error.code(),
                    "message": error.to_string(),
                    "exit_code": error.exit_code(),
                }
            });
            if let Some(hint) = error.hint() {
                payload["error"]["hint"] = json!(hint);
            }
            println!("{payload}");
        } else {
            eprintln!("{} {error}", self.red("error:"));
            if let Some(hint) = error.hint() {
                eprintln!("       {}", self.dim(hint));
            }
        }
    }

    /// Report a failure that carries its own remediation steps.
    pub fn fail(&self, code: &str, exit_code: i32, message: &str, fixes: &[String]) {
        if self.json {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": {
                        "code": code,
                        "message": message,
                        "exit_code": exit_code,
                        "fixes": fixes,
                    }
                })
            );
        } else {
            eprintln!("{} {message}", self.red("error:"));
            if !fixes.is_empty() {
                eprintln!();
                eprintln!("{}", self.bold("Try this:"));
                for fix in fixes {
                    eprintln!("  • {fix}");
                }
            }
        }
    }

    pub fn bold(&self, text: impl AsRef<str>) -> String {
        self.paint("1", text.as_ref())
    }

    pub fn dim(&self, text: impl AsRef<str>) -> String {
        self.paint("2", text.as_ref())
    }

    pub fn green(&self, text: impl AsRef<str>) -> String {
        self.paint("32", text.as_ref())
    }

    pub fn red(&self, text: impl AsRef<str>) -> String {
        self.paint("31", text.as_ref())
    }

    pub fn yellow(&self, text: impl AsRef<str>) -> String {
        self.paint("33", text.as_ref())
    }

    pub fn cyan(&self, text: impl AsRef<str>) -> String {
        self.paint("36", text.as_ref())
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    /// Left-aligned columns with a dim header row.
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>]) -> String {
        let mut widths: Vec<usize> = headers
            .iter()
            .map(|header| header.chars().count())
            .collect();
        for row in rows {
            for (index, cell) in row.iter().enumerate() {
                if index < widths.len() {
                    widths[index] = widths[index].max(cell.chars().count());
                }
            }
        }
        let mut out = String::new();
        let header = headers
            .iter()
            .enumerate()
            .map(|(index, text)| pad(text, widths[index]))
            .collect::<Vec<_>>()
            .join("  ");
        out.push_str(&self.dim(header.trim_end()));
        out.push('\n');
        for row in rows {
            let line = row
                .iter()
                .enumerate()
                .map(|(index, cell)| pad(cell, widths.get(index).copied().unwrap_or(0)))
                .collect::<Vec<_>>()
                .join("  ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    let current = text.chars().count();
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
}

fn render_event(event: &Value) -> String {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("event");
    let rest = match event {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| key.as_str() != "type")
            .map(|(key, value)| format!("{key}={}", plain(value)))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    if rest.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {rest}")
    }
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
