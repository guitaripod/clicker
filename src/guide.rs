//! The cheat sheet behind `clicker guide`, and the "try this" block that the
//! setup flow ends with.

use serde_json::{json, Value};

use crate::output::Output;

pub struct Section {
    pub title: &'static str,
    pub items: &'static [(&'static str, &'static str)],
}

pub const SECTIONS: &[Section] = &[
    Section {
        title: "Getting started",
        items: &[
            ("clicker setup", "find your Apple TV and pair with it"),
            ("clicker scan", "list Apple TVs on this network"),
            ("clicker devices", "show the devices you have paired"),
            (
                "clicker doctor",
                "check discovery, credentials and connections",
            ),
        ],
    },
    Section {
        title: "Everyday control",
        items: &[
            ("clicker press up|down|left|right", "move around the screen"),
            ("clicker press select", "press the touchpad"),
            ("clicker press menu", "go back"),
            ("clicker press home", "open the home screen"),
            ("clicker press play-pause", "toggle playback"),
            ("clicker volume up|down", "one volume step"),
            ("clicker power on|off", "wake or sleep the Apple TV"),
        ],
    },
    Section {
        title: "Touchpad and typing",
        items: &[
            (
                "clicker touch swipe --from 500,900 --to 500,100",
                "swipe up, coordinates run 0..1000",
            ),
            ("clicker touch click", "click the touchpad"),
            (
                "clicker text set \"blade runner\"",
                "type into a focused search field",
            ),
            ("clicker text get", "read the current text"),
        ],
    },
    Section {
        title: "Apps and accounts",
        items: &[
            ("clicker apps list", "installed apps with bundle ids"),
            ("clicker apps launch com.netflix.Netflix", "open an app"),
            ("clicker accounts list", "Apple TV user accounts"),
        ],
    },
    Section {
        title: "For scripts and agents",
        items: &[
            ("clicker status --json", "one JSON object, stable keys"),
            ("clicker watch --json", "stream device events as NDJSON"),
            (
                "clicker batch --json",
                "run NDJSON commands over one connection",
            ),
            (
                "clicker raw _hidC --content '{\"_hBtS\":1,\"_hidC\":6}'",
                "send anything",
            ),
            ("clicker completions bash", "shell completions"),
        ],
    },
    Section {
        title: "When something is wrong",
        items: &[
            ("clicker doctor", "what works and what does not"),
            ("clicker scan --all", "include devices that look unusual"),
            (
                "clicker forget \"Apple TV\"",
                "drop credentials and start over",
            ),
            ("clicker setup --force", "pair again"),
        ],
    },
];

pub fn render(out: &Output) {
    let sections: Vec<Value> = SECTIONS
        .iter()
        .map(|section| {
            json!({
                "title": section.title,
                "items": section
                    .items
                    .iter()
                    .map(|(command, description)| json!({
                        "command": command,
                        "description": description,
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    out.ok(json!({
        "sections": sections,
        "docs": "https://github.com/guitaripod/clicker",
    }));

    let mut text = String::new();
    for (index, section) in SECTIONS.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        text.push_str(&out.bold(section.title));
        text.push('\n');
        let width = section
            .items
            .iter()
            .map(|(command, _)| command.chars().count())
            .max()
            .unwrap_or(0);
        for (command, description) in section.items {
            text.push_str(&format!(
                "  {}  {}\n",
                out.cyan(pad(command, width)),
                out.dim(description)
            ));
        }
    }
    out.text(text.trim_end());
}

pub fn next_steps(out: &Output) {
    let steps = [
        ("clicker press home", "go to the home screen"),
        ("clicker status", "power, volume and tvOS version"),
        ("clicker tui", "use the whole remote on screen"),
        ("clicker guide", "the full cheat sheet"),
    ];
    let mut text = format!("{}\n", out.bold("Try this next:"));
    for (command, description) in steps {
        text.push_str(&format!(
            "  {}  {}\n",
            out.cyan(pad(command, 22)),
            out.dim(description)
        ));
    }
    out.text(text.trim_end());
}

fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    let current = text.chars().count();
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
}
