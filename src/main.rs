use std::io::{BufRead, IsTerminal as _, Write};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{json, Value};

use clicker::app::Context;
use clicker::companion::client::Client;
use clicker::companion::keys::{Gesture, HidCommand, InputAction, MediaCommand, PowerState};
use clicker::discovery;
use clicker::output::Output;
use clicker::store::Store;
use clicker::{Error, Result};

#[derive(Parser)]
#[command(
    name = "clicker",
    about = "Apple TV remote for the terminal",
    long_about = "Apple TV remote for the terminal: buttons, touch, typing, apps and power.\n\
                  Speaks Apple's Companion protocol directly over the local network, with no app\n\
                  on the TV and no cloud in between.",
    version,
    after_help = "Start here:\n\
                  \x20 clicker setup      find and pair your Apple TV (about a minute)\n\
                  \x20 clicker guide      cheat sheet of everything below\n\
                  \x20 clicker doctor     diagnose discovery, credentials and connections\n\
                  \n\
                  Every command accepts --json and returns one object with \"ok\".\n\
                  Exit codes: 0 ok, 2 usage, 3 no such device, 4 not paired, 5 auth, 6 timeout, 7 protocol, 8 config."
)]
struct Cli {
    /// Emit machine-readable JSON instead of text (env: CLICKER_JSON)
    #[arg(long, global = true)]
    json: bool,

    /// Device to talk to: id, id prefix, name or address (env: CLICKER_DEVICE)
    #[arg(short, long, global = true)]
    device: Option<String>,

    /// Seconds to wait for any single device response
    #[arg(long, global = true, default_value_t = 5.0)]
    timeout: f64,

    /// Log protocol traffic to stderr (repeat for more)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find and pair your Apple TV, the guided way
    ///
    /// Walks through discovery, pairing and a connection check, in ordinary
    /// language. Use this the first time; use `clicker pair` in scripts.
    ///
    #[command(
        after_help = "Examples:\n  clicker setup\n  clicker setup --device \"Apple TV\"\n  clicker setup --force\n  clicker setup --json"
    )]
    Setup {
        /// PIN shown on the TV; omit to type it when asked
        #[arg(long)]
        pin: Option<String>,
        /// Name this remote reports to the Apple TV
        #[arg(long, default_value = "Clicker")]
        name: String,
        /// Pair again even if this device already has credentials
        #[arg(long)]
        force: bool,
    },

    /// Print a cheat sheet of every command
    ///
    /// Grouped by task, from pressing a button to streaming events from a
    /// script. `clicker guide --json` returns the same content as data.
    Guide,

    /// Discover Apple TVs on the local network
    ///
    /// Listens for Bonjour for two seconds. Devices marked with `~` answered
    /// without a matching AirPlay record, which usually means a Mac
    /// advertising the Remote service rather than an Apple TV.
    ///
    #[command(
        after_help = "Examples:\n  clicker scan\n  clicker scan --all\n  clicker scan --json --timeout 5"
    )]
    Scan {
        /// Seconds to listen for Bonjour records
        #[arg(long, default_value_t = 2.0)]
        timeout: f64,
        /// Include records that do not look like an Apple TV
        #[arg(long)]
        all: bool,
    },

    /// Pair with an Apple TV using the PIN it displays
    ///
    /// The Apple TV shows a new 4-digit PIN for every attempt. Without `--pin`
    /// the PIN is read from stdin, which also works when another program is
    /// driving this one.
    ///
    #[command(
        after_help = "Examples:\n  clicker pair --device \"Apple TV\"\n  echo 4821 | clicker pair --device \"Apple TV\" --json"
    )]
    Pair {
        /// PIN shown on the TV; omit to read it from stdin
        #[arg(long)]
        pin: Option<String>,
        /// Name this remote reports to the Apple TV
        #[arg(long, default_value = "Clicker")]
        name: String,
        /// Re-pair a device that already has credentials
        #[arg(long)]
        force: bool,
    },

    /// List paired devices
    Devices,

    /// Remove stored credentials for a device
    Forget {
        /// Device to forget; defaults to the one --device selects
        device: Option<String>,
    },

    /// Press one or more remote buttons
    ///
    /// Keys run in order, with a short pause between them. `--action hold`
    /// keeps the button down for `--hold-ms`.
    ///
    #[command(
        after_help = "Examples:\n  clicker press down down select\n  clicker press home\n  clicker press menu --action hold\n  clicker press down --repeat 5"
    )]
    Press {
        /// Button names; see `clicker keys`
        #[arg(required = true)]
        keys: Vec<String>,
        /// single, double or hold
        #[arg(long, default_value = "single")]
        action: String,
        /// Hold duration in milliseconds
        #[arg(long, default_value_t = 1000)]
        hold_ms: u64,
        /// Repeat the whole sequence N times
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        /// Pause between buttons and repeats, in milliseconds
        #[arg(long, default_value_t = 120)]
        interval_ms: u64,
    },

    /// Transport controls
    ///
    /// Unlike `press play-pause`, which is the remote's own button, this talks
    /// to the media session directly.
    ///
    #[command(
        after_help = "Examples:\n  clicker media next\n  clicker media skip 10\n  clicker media skip -15"
    )]
    Media {
        #[command(subcommand)]
        action: MediaAction,
    },

    /// Volume control
    ///
    /// Works when the Apple TV controls the volume; if the TV or a receiver
    /// does, the Apple TV reports `external` and the buttons still act as
    /// volume steps for whatever it is paired with.
    ///
    #[command(
        after_help = "Examples:\n  clicker volume\n  clicker volume set 30\n  clicker volume up"
    )]
    Volume {
        #[command(subcommand)]
        action: VolumeAction,
    },

    /// Wake, sleep or read the power state
    Power {
        #[command(subcommand)]
        action: PowerAction,
    },

    /// List installed apps or launch one
    ///
    #[command(
        after_help = "Examples:\n  clicker apps list\n  clicker apps launch com.netflix.Netflix"
    )]
    Apps {
        #[command(subcommand)]
        action: AppsAction,
    },

    /// Apple TV user accounts
    Accounts {
        #[command(subcommand)]
        action: AccountsAction,
    },

    /// Type into the text field that is focused on the TV
    ///
    /// The Apple TV must have a keyboard on screen (open a search field first).
    /// `set` replaces the contents, `append` adds at the insertion point.
    ///
    #[command(
        after_help = "Examples:\n  clicker text set \"blade runner\"\n  clicker text get\n  clicker text clear"
    )]
    Text {
        #[command(subcommand)]
        action: TextAction,
    },

    /// Touchpad gestures
    ///
    /// Coordinates run from 0 to 1000 on both axes, with (0, 0) at the
    /// top-left, so swiping up is `--from 500,900 --to 500,100`.
    ///
    #[command(
        after_help = "Examples:\n  clicker touch swipe --from 500,900 --to 500,100 --duration-ms 250\n  clicker touch tap 500,500\n  clicker touch click --double"
    )]
    Touch {
        #[command(subcommand)]
        action: TouchAction,
    },

    /// Power, volume and playback capabilities in one call
    ///
    /// The cheapest way to check that a device is alive and what it supports.
    Status,

    /// Stream device events as one JSON object per line
    ///
    /// Emits `power`, `media_control` and `text_focus` events until the
    /// duration expires or the process is interrupted.
    ///
    #[command(
        after_help = "Examples:\n  clicker watch --json\n  clicker watch --json --duration 30\n  clicker watch --json --events SystemStatus,_iMC"
    )]
    Watch {
        /// Stop after N seconds instead of running until interrupted
        #[arg(long)]
        duration: Option<f64>,
        /// Comma-separated event names to register (default: all known)
        #[arg(long, value_delimiter = ',')]
        events: Option<Vec<String>>,
    },

    /// Send a raw Companion command or event
    ///
    /// The escape hatch for anything this CLI does not wrap yet: pass the
    /// identifier and an optional JSON body, and get the `_c` payload back.
    ///
    #[command(
        after_help = "Examples:\n  clicker raw FetchAttentionState\n  clicker raw _hidC --content {\"_hBtS\":1,\"_hidC\":6}"
    )]
    Raw {
        /// Command identifier, for example `_hidC` or `FetchAttentionState`
        identifier: String,
        /// JSON object passed as `_c`
        #[arg(long)]
        content: Option<String>,
        /// Send as an event (`_t` 1) instead of a request (`_t` 2)
        #[arg(long)]
        event: bool,
    },

    /// List button names accepted by `clicker press`
    Keys,

    /// Check discovery, credentials and connectivity
    ///
    /// Runs every precondition this tool depends on and says what to do about
    /// the ones that fail. Safe to run before pairing.
    Doctor,

    /// The whole remote on screen, with Vim keys
    ///
    /// h j k l press the arrows, enter selects, m is back, gg goes home.
    /// Counts work (5j), so do macros (qa … q, then @a) and `.` repeats the
    /// last change. `:` opens a command line, `?` lists every key, `:q` quits.
    #[command(
        after_help = "Example:\n  clicker tui\n\nInside: h j k l, enter, m, gg, p, i, o, ?, :vol 40, :app net, :q"
    )]
    Tui,

    /// Read newline-delimited JSON commands and run them over one connection
    ///
    /// One process per button costs a connection and a handshake; batch keeps
    /// a single session for a whole sequence. Reads stdin, writes NDJSON.
    ///
    /// Ops: press, text, volume, power, apps, app, raw, sleep.
    ///
    #[command(
        after_help = "Example:\n  printf '%s\\n' '{\"op\":\"press\",\"keys\":[\"down\",\"select\"]}' | clicker batch --json"
    )]
    Batch,

    /// Print shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum MediaAction {
    /// Play
    Play,
    /// Pause
    Pause,
    /// Toggle play/pause with the remote's play button
    PlayPause,
    /// Next track or item
    Next,
    /// Previous track or item
    Previous,
    /// Skip by N seconds, negative to go back
    Skip {
        /// Seconds to jump; negative rewinds
        #[arg(allow_hyphen_values = true)]
        seconds: f64,
    },
    /// Fast forward while held
    FastForward,
    /// Rewind while held
    Rewind,
}

#[derive(Subcommand)]
enum VolumeAction {
    /// Read the current volume
    Get,
    /// Set the volume in percent
    Set {
        /// 0 to 100
        percent: f64,
    },
    /// One step louder
    Up,
    /// One step quieter
    Down,
}

#[derive(Subcommand)]
enum PowerAction {
    /// Wake the Apple TV
    On,
    /// Put the Apple TV to sleep
    Off,
    /// Read the current power state
    Status,
}

#[derive(Subcommand)]
enum AppsAction {
    /// List installed apps
    List,
    /// Open an app by bundle id, or a URL
    Launch {
        /// Bundle id such as com.netflix.Netflix, or a URL
        target: String,
    },
}

#[derive(Subcommand)]
enum AccountsAction {
    /// List Apple TV user accounts
    List,
    /// Switch to another account
    Switch {
        /// Account id from `clicker accounts list`
        id: String,
    },
}

#[derive(Subcommand)]
enum TextAction {
    /// Replace the field contents
    Set {
        /// Text to type
        text: String,
    },
    /// Append at the insertion point
    Append {
        /// Text to add
        text: String,
    },
    /// Empty the field
    Clear,
    /// Print the current text
    Get,
}

#[derive(Subcommand)]
enum TouchAction {
    /// Drag across the touchpad
    Swipe {
        /// Start point as x,y from 0 to 1000
        #[arg(long, value_parser = parse_point)]
        from: (i64, i64),
        /// End point as x,y from 0 to 1000
        #[arg(long, value_parser = parse_point)]
        to: (i64, i64),
        /// Milliseconds the drag takes
        #[arg(long, default_value_t = 300)]
        duration_ms: u64,
    },
    /// Touch and release at a point
    Tap {
        /// Point as x,y from 0 to 1000
        #[arg(value_parser = parse_point)]
        at: (i64, i64),
    },
    /// Touch and hold at a point
    Hold {
        /// Point as x,y from 0 to 1000
        #[arg(value_parser = parse_point)]
        at: (i64, i64),
        /// Milliseconds to hold
        #[arg(long, default_value_t = 800)]
        duration_ms: u64,
    },
    /// Click the touchpad at its centre
    Click {
        /// Two quick clicks
        #[arg(long)]
        double: bool,
    },
}

/// Behave like a normal Unix tool when stdout closes early: `clicker apps list
/// | head` should stop quietly rather than panic on a broken pipe.
fn restore_sigpipe() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Truthy environment flag: 1, true, yes and on all count.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn device_from_env() -> Option<String> {
    std::env::var("CLICKER_DEVICE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_point(value: &str) -> std::result::Result<(i64, i64), String> {
    let (x, y) = value
        .split_once(',')
        .ok_or_else(|| "expected x,y".to_string())?;
    Ok((
        x.trim()
            .parse()
            .map_err(|_| "x must be an integer".to_string())?,
        y.trim()
            .parse()
            .map_err(|_| "y must be an integer".to_string())?,
    ))
}

#[tokio::main]
async fn main() {
    restore_sigpipe();
    let mut cli = Cli::parse();
    cli.json = cli.json || env_flag("CLICKER_JSON");
    if cli.device.as_deref().unwrap_or_default().is_empty() {
        cli.device = Some(device_from_env()).flatten();
    }
    clicker::log::set_level(clicker::log::level_from_env().unwrap_or(cli.verbose));

    let out = Output::new(cli.json);
    let code = match run(cli).await {
        Ok(()) => 0,
        Err(error) => {
            if !matches!(error, Error::Reported { .. }) {
                out.error(&error);
            }
            error.exit_code()
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<()> {
    if let Command::Completions { shell } = cli.command {
        let mut command = Cli::command();
        clap_complete::generate(shell, &mut command, "clicker", &mut std::io::stdout());
        return Ok(());
    }
    if let Command::Guide = cli.command {
        clicker::guide::render(&Output::new(cli.json));
        return Ok(());
    }
    if let Command::Keys = cli.command {
        let out = Output::new(cli.json);
        let keys: Vec<&str> = HidCommand::ALL.iter().map(|key| key.name()).collect();
        out.ok(json!({ "keys": keys }));
        out.text(keys.join("\n"));
        return Ok(());
    }

    let store = Store::load()?;
    let mut ctx = Context {
        out: Output::new(cli.json),
        store,
        selector: cli.device.clone(),
        timeout: Duration::from_secs_f64(cli.timeout.max(0.2)),
    };

    match cli.command {
        Command::Setup { pin, name, force } => {
            clicker::setup::run(&mut ctx, clicker::setup::Options { pin, name, force }).await
        }
        Command::Guide => unreachable!(),
        Command::Scan { timeout, all } => scan(&ctx, timeout, all).await,
        Command::Pair { pin, name, force } => pair(&mut ctx, pin, name, force).await,
        Command::Devices => devices(&ctx),
        Command::Forget { device } => forget(&mut ctx, device),
        Command::Press {
            keys,
            action,
            hold_ms,
            repeat,
            interval_ms,
        } => press(&mut ctx, keys, action, hold_ms, repeat, interval_ms).await,
        Command::Media { action } => media(&mut ctx, action).await,
        Command::Volume { action } => volume(&mut ctx, action).await,
        Command::Power { action } => power(&mut ctx, action).await,
        Command::Apps { action } => apps(&mut ctx, action).await,
        Command::Accounts { action } => accounts(&mut ctx, action).await,
        Command::Text { action } => text(&mut ctx, action).await,
        Command::Touch { action } => touch(&mut ctx, action).await,
        Command::Status => status(&mut ctx).await,
        Command::Watch { duration, events } => watch(&mut ctx, duration, events).await,
        Command::Raw {
            identifier,
            content,
            event,
        } => raw(&mut ctx, identifier, content, event).await,
        Command::Doctor => doctor(&mut ctx).await,
        Command::Tui => {
            let (device, client) = ctx.connect().await?;
            clicker::tui::run(client, &device.name).await
        }
        Command::Batch => batch(&mut ctx).await,
        Command::Completions { .. } | Command::Keys => unreachable!(),
    }
}

async fn scan(ctx: &Context, timeout: f64, all: bool) -> Result<()> {
    let devices = discovery::scan(Duration::from_secs_f64(timeout.max(0.2)), all).await?;
    let mut listed = Vec::new();
    for device in devices {
        let paired = ctx
            .store
            .devices()
            .iter()
            .any(|stored| stored.id == device.id);
        let mut value = serde_json::to_value(&device).unwrap_or(Value::Null);
        value["paired"] = json!(paired);
        listed.push(value);
    }

    ctx.out.ok(json!({ "devices": listed }));

    if listed.is_empty() {
        ctx.out.text(format!(
            "{}\n\n{}",
            ctx.out.bold("No Apple TVs answered."),
            ctx.out.dim(
                "Check that the TV is awake, on the same network, and that AirPlay is on.\n\
                 Then try `clicker setup`, which explains each step."
            )
        ));
        return Ok(());
    }

    let rows: Vec<Vec<String>> = listed
        .iter()
        .map(|device| {
            let field = |key: &str| {
                device
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            vec![
                format!(
                    "{}{}",
                    if device
                        .get("airplay_confirmed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        ""
                    } else {
                        "~"
                    },
                    field("name")
                ),
                field("model"),
                field("os_version"),
                field("address"),
                if device
                    .get("paired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    ctx.out.green("paired")
                } else {
                    ctx.out.dim("not paired")
                },
            ]
        })
        .collect();
    ctx.out.text(
        ctx.out
            .table(&["DEVICE", "MODEL", "TVOS", "ADDRESS", "STATUS"], &rows),
    );
    Ok(())
}

async fn pair(ctx: &mut Context, pin: Option<String>, name: String, force: bool) -> Result<()> {
    let found = discovery::scan_until(Duration::from_secs(6), false, |devices| {
        devices
            .iter()
            .any(|device| device.airplay_confirmed && !device.local)
    })
    .await?;
    let pairable: Vec<_> = found
        .into_iter()
        .filter(|device| device.pairable && !device.local)
        .collect();
    let confirmed: Vec<_> = pairable
        .iter()
        .filter(|device| device.airplay_confirmed)
        .cloned()
        .collect();
    let candidates = if confirmed.is_empty() {
        pairable
    } else {
        confirmed
    };
    let target = match &ctx.selector {
        Some(selector) => {
            let needle = selector.to_ascii_lowercase();
            candidates
                .iter()
                .find(|device| {
                    device.id.to_ascii_lowercase() == needle
                        || device.id.to_ascii_lowercase().starts_with(&needle)
                        || device.name.to_ascii_lowercase().contains(&needle)
                        || device.address == *selector
                })
                .cloned()
                .ok_or_else(|| Error::DeviceNotFound(selector.clone()))?
        }
        None => match candidates.len() {
            1 => candidates[0].clone(),
            0 => {
                return Err(Error::DeviceNotFound(
                    "a pairable Apple TV on this network".into(),
                ))
            }
            _ => {
                let names: Vec<String> = candidates
                    .iter()
                    .map(|device| format!("{} ({})", device.name, device.id))
                    .collect();
                return Err(Error::Usage(format!(
                    "several Apple TVs found; pass --device: {}",
                    names.join(", ")
                )));
            }
        },
    };

    if !force
        && ctx
            .store
            .devices()
            .iter()
            .any(|device| device.id == target.id)
    {
        return Err(Error::Usage(format!(
            "{} is already paired; pass --force to pair again",
            target.name
        )));
    }

    let mut conn =
        clicker::companion::conn::Connection::open(&target.address, target.port, ctx.timeout)
            .await?;

    // The Apple TV only shows a PIN once pairing has started, so begin the
    // exchange before asking for it.
    let setup = clicker::hap::PairSetup::begin(&mut conn).await?;

    let pin = match pin {
        Some(pin) => pin,
        None => {
            ctx.out.stream(json!({
                "type": "pin_required",
                "device": target.name,
                "message": "enter the PIN shown on the Apple TV",
            }));
            if !ctx.out.json {
                eprint!("PIN shown on {}: ", target.name);
                std::io::stderr().flush().ok();
            }
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            if !std::io::stdin().is_terminal() {
                eprintln!();
            }
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                return Err(Error::Usage("no PIN entered".into()));
            }
            trimmed
        }
    };

    let credentials = setup.finish(&mut conn, &name, &pin).await?;
    drop(conn);

    let device = Store::from_discovery(&target, credentials, &name);
    ctx.store.upsert(device.clone());
    ctx.store.save()?;

    ctx.out.ok(json!({
        "device": Context::device_json(&device),
        "display_name": device.display_name,
    }));
    ctx.out.text(format!(
        "paired with {} at {}:{} as \"{}\"",
        device.name, device.address, device.port, device.display_name
    ));
    Ok(())
}

fn devices(ctx: &Context) -> Result<()> {
    let devices: Vec<Value> = ctx
        .store
        .devices()
        .iter()
        .map(Context::device_json)
        .collect();
    ctx.out.ok(json!({ "devices": devices }));

    if ctx.store.devices().is_empty() {
        ctx.out.text(format!(
            "{}\n\n{}",
            ctx.out.bold("Nothing paired yet."),
            ctx.out
                .dim("Run `clicker setup` to find your Apple TV and pair with it.")
        ));
        return Ok(());
    }

    let rows: Vec<Vec<String>> = ctx
        .store
        .devices()
        .iter()
        .map(|device| {
            vec![
                device.name.clone(),
                device.model.clone().unwrap_or_else(|| "-".into()),
                device.address.clone(),
                device.display_name.clone(),
            ]
        })
        .collect();
    ctx.out.text(
        ctx.out
            .table(&["DEVICE", "MODEL", "ADDRESS", "REMOTE NAME"], &rows),
    );
    Ok(())
}

fn forget(ctx: &mut Context, device: Option<String>) -> Result<()> {
    let selector = device
        .or_else(|| ctx.selector.clone())
        .ok_or_else(|| Error::Usage("pass a device to forget".into()))?;
    let removed = ctx.store.remove(&selector)?;
    ctx.store.save()?;
    ctx.out.ok(json!({
        "forgotten": { "id": removed.id, "name": removed.name },
        "config": ctx.store.path().display().to_string(),
    }));
    ctx.out
        .text(format!("forgot {} ({})", removed.name, removed.id));
    Ok(())
}

async fn press(
    ctx: &mut Context,
    keys: Vec<String>,
    action: String,
    hold_ms: u64,
    repeat: u32,
    interval_ms: u64,
) -> Result<()> {
    let action = InputAction::parse(&action)
        .ok_or_else(|| Error::Usage(format!("unknown action \"{action}\"")))?;
    let commands = keys
        .iter()
        .map(|key| HidCommand::parse(key).ok_or_else(|| unknown_button(key)))
        .collect::<Result<Vec<_>>>()?;

    let (device, client) = ctx.connect().await?;
    let hold = Duration::from_millis(hold_ms);
    for round in 0..repeat.max(1) {
        for command in &commands {
            client.press(*command, action, hold).await?;
            if interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        }
        if round + 1 < repeat.max(1) && interval_ms > 0 {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    let names: Vec<&str> = commands.iter().map(|command| command.name()).collect();
    ctx.out.ok(json!({
        "device": device.name,
        "keys": names,
        "action": action_name(action),
        "repeat": repeat.max(1),
    }));
    let mut summary = names.join(" ");
    match action {
        InputAction::Hold => summary.push_str(&format!(" (hold {hold_ms}ms)")),
        InputAction::Double => summary.push_str(" (double)"),
        InputAction::Single => {}
    }
    if repeat > 1 {
        summary.push_str(&format!(" x{repeat}"));
    }
    ctx.out.text(summary);
    Ok(())
}

/// The error for a button name we do not know, with the nearest match.
fn unknown_button(key: &str) -> Error {
    let names: Vec<&str> = HidCommand::ALL
        .iter()
        .map(|command| command.name())
        .collect();
    let suggestion = closest(key, &names);
    let message = match suggestion {
        Some(name) => format!(
            "\"{key}\" is not a button; did you mean \"{name}\"? `clicker keys` lists them all"
        ),
        None => format!("\"{key}\" is not a button; `clicker keys` lists them all"),
    };
    Error::Usage(message)
}

fn closest<'a>(needle: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let needle = needle.to_ascii_lowercase();
    let limit = if needle.chars().count() >= 4 { 2 } else { 1 };
    candidates
        .iter()
        .map(|candidate| (*candidate, distance(&needle, candidate)))
        .filter(|(_, distance)| *distance <= limit)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate)
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn action_name(action: InputAction) -> &'static str {
    match action {
        InputAction::Single => "single",
        InputAction::Double => "double",
        InputAction::Hold => "hold",
    }
}

async fn media(ctx: &mut Context, action: MediaAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    let label = match action {
        MediaAction::Play => {
            client.media(MediaCommand::Play, vec![]).await?;
            "play"
        }
        MediaAction::Pause => {
            client.media(MediaCommand::Pause, vec![]).await?;
            "pause"
        }
        MediaAction::PlayPause => {
            client
                .press(HidCommand::PlayPause, InputAction::Single, Duration::ZERO)
                .await?;
            "play-pause"
        }
        MediaAction::Next => {
            client.media(MediaCommand::NextTrack, vec![]).await?;
            "next"
        }
        MediaAction::Previous => {
            client.media(MediaCommand::PreviousTrack, vec![]).await?;
            "previous"
        }
        MediaAction::Skip { seconds } => {
            client.skip(seconds).await?;
            "skip"
        }
        MediaAction::FastForward => {
            client.media(MediaCommand::FastForwardBegin, vec![]).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            client.media(MediaCommand::FastForwardEnd, vec![]).await?;
            "fast-forward"
        }
        MediaAction::Rewind => {
            client.media(MediaCommand::RewindBegin, vec![]).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            client.media(MediaCommand::RewindEnd, vec![]).await?;
            "rewind"
        }
    };
    ctx.out
        .ok(json!({ "device": device.name, "action": label }));
    ctx.out.text(label);
    Ok(())
}

async fn volume(ctx: &mut Context, action: VolumeAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    match action {
        VolumeAction::Get => {
            let level = client.volume().await?;
            ctx.out.ok(json!({ "device": device.name, "level": level }));
            ctx.out.text(match level {
                Some(level) => format!("{level:.0}%"),
                None => "volume is controlled by the TV or receiver".into(),
            });
        }
        VolumeAction::Set { percent } => {
            if !(0.0..=100.0).contains(&percent) {
                return Err(Error::Usage("volume must be between 0 and 100".into()));
            }
            client.set_volume(percent).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let level = client.volume().await?.unwrap_or(percent);
            ctx.out.ok(json!({ "device": device.name, "level": level }));
            ctx.out.text(format!("{level:.0}%"));
        }
        VolumeAction::Up => {
            client
                .press(HidCommand::VolumeUp, InputAction::Single, Duration::ZERO)
                .await?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let level = client.volume().await?;
            ctx.out.ok(json!({ "device": device.name, "level": level }));
            ctx.out.text(match level {
                Some(level) => format!("{level:.0}%"),
                None => "volume up".into(),
            });
        }
        VolumeAction::Down => {
            client
                .press(HidCommand::VolumeDown, InputAction::Single, Duration::ZERO)
                .await?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let level = client.volume().await?;
            ctx.out.ok(json!({ "device": device.name, "level": level }));
            ctx.out.text(match level {
                Some(level) => format!("{level:.0}%"),
                None => "volume down".into(),
            });
        }
    }
    Ok(())
}

async fn power(ctx: &mut Context, action: PowerAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    let state = match action {
        PowerAction::On => {
            client
                .press(HidCommand::Wake, InputAction::Single, Duration::ZERO)
                .await?;
            PowerState::On
        }
        PowerAction::Off => {
            client
                .press(HidCommand::Sleep, InputAction::Single, Duration::ZERO)
                .await?;
            PowerState::Off
        }
        PowerAction::Status => client.power_state().await,
    };
    ctx.out
        .ok(json!({ "device": device.name, "state": state.as_str() }));
    ctx.out.text(state.as_str());
    Ok(())
}

async fn apps(ctx: &mut Context, action: AppsAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    match action {
        AppsAction::List => {
            let apps = client.app_list().await?;
            let list: Vec<Value> = apps
                .iter()
                .map(|(bundle_id, name)| json!({ "bundle_id": bundle_id, "name": name }))
                .collect();
            ctx.out.ok(json!({ "device": device.name, "apps": list }));
            let text = apps
                .iter()
                .map(|(bundle_id, name)| format!("{name:<28} {bundle_id}"))
                .collect::<Vec<_>>()
                .join("\n");
            ctx.out.text(text);
        }
        AppsAction::Launch { target } => {
            client.launch_app(&target).await?;
            ctx.out
                .ok(json!({ "device": device.name, "launched": target }));
            ctx.out.text(format!("launched {target}"));
        }
    }
    Ok(())
}

async fn accounts(ctx: &mut Context, action: AccountsAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    match action {
        AccountsAction::List => {
            let accounts = client.account_list().await?;
            let list: Vec<Value> = accounts
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect();
            ctx.out
                .ok(json!({ "device": device.name, "accounts": list }));
            ctx.out.text(
                accounts
                    .iter()
                    .map(|(id, name)| format!("{name:<24} {id}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        AccountsAction::Switch { id } => {
            client.switch_account(&id).await?;
            ctx.out
                .ok(json!({ "device": device.name, "switched_to": id }));
            ctx.out.text(format!("switched to {id}"));
        }
    }
    Ok(())
}

async fn text(ctx: &mut Context, action: TextAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    match action {
        TextAction::Set { text } => {
            let value = client.text_set(&text).await?;
            ctx.out.ok(json!({ "device": device.name, "text": value }));
            ctx.out.text(value);
        }
        TextAction::Append { text } => {
            let value = client.text_append(&text).await?;
            ctx.out.ok(json!({ "device": device.name, "text": value }));
            ctx.out.text(value);
        }
        TextAction::Clear => {
            client.text_clear().await?;
            ctx.out.ok(json!({ "device": device.name, "text": "" }));
            ctx.out.text("cleared");
        }
        TextAction::Get => {
            let value = client.text_get().await?;
            ctx.out.ok(json!({ "device": device.name, "text": value }));
            ctx.out.text(&value);
        }
    }
    Ok(())
}

async fn touch(ctx: &mut Context, action: TouchAction) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    match action {
        TouchAction::Swipe {
            from,
            to,
            duration_ms,
        } => {
            let gesture = Gesture {
                from,
                to,
                duration: Duration::from_millis(duration_ms),
            };
            client.swipe(gesture).await?;
            ctx.out.ok(json!({
                "device": device.name,
                "gesture": "swipe",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "duration_ms": duration_ms,
            }));
            ctx.out.text(format!("swipe {gesture}"));
        }
        TouchAction::Tap { at } => {
            client
                .touch(at.0, at.1, clicker::companion::keys::TouchPhase::Press)
                .await?;
            tokio::time::sleep(Duration::from_millis(40)).await;
            client
                .touch(at.0, at.1, clicker::companion::keys::TouchPhase::Release)
                .await?;
            ctx.out
                .ok(json!({ "device": device.name, "gesture": "tap", "at": [at.0, at.1] }));
            ctx.out.text(format!("tap at {},{}", at.0, at.1));
        }
        TouchAction::Hold { at, duration_ms } => {
            client
                .touch(at.0, at.1, clicker::companion::keys::TouchPhase::Press)
                .await?;
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
            client
                .touch(at.0, at.1, clicker::companion::keys::TouchPhase::Release)
                .await?;
            ctx.out.ok(json!({
                "device": device.name,
                "gesture": "hold",
                "at": [at.0, at.1],
                "duration_ms": duration_ms,
            }));
            ctx.out
                .text(format!("hold at {},{} for {duration_ms}ms", at.0, at.1));
        }
        TouchAction::Click { double } => {
            let action = if double {
                InputAction::Double
            } else {
                InputAction::Single
            };
            client.click(action).await?;
            ctx.out.ok(json!({
                "device": device.name,
                "gesture": if double { "double-click" } else { "click" },
            }));
            ctx.out.text(if double { "double click" } else { "click" });
        }
    }
    Ok(())
}

async fn status(ctx: &mut Context) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    let power = client.power_state().await;
    let flags = match client.media_flags {
        Some(flags) => Some(flags),
        None => client.media_flags(Duration::from_millis(900)).await,
    };
    // The flags under-report capability (tvOS reports 0 while an app is idle
    // and still accepts volume commands), so ask the device directly.
    let volume = client.volume().await.ok().flatten();

    ctx.out.ok(json!({
        "device": Context::device_json(&device),
        "reported_name": client.info.name,
        "os_version": client.info.os_version,
        "power": power.as_str(),
        "volume": volume,
        "media_control": {
            "flags": flags,
            "volume_control": volume.is_some() || flags.map(|f| f & 0x0100 != 0).unwrap_or(false),
            "play": flags.map(|f| f & 0x0001 != 0),
            "skip": flags.map(|f| f & 0x0200 != 0),
        },
    }));

    let mut text = format!(
        "{}\n  {}  {}\n  {}  {}\n  {}  {}\n  {}  {}\n  {}  {}",
        ctx.out.bold(&device.name),
        ctx.out.dim("power  "),
        power.as_str(),
        ctx.out.dim("volume "),
        match volume {
            Some(level) => format!("{level:.0}%"),
            None => "controlled by the TV".into(),
        },
        ctx.out.dim("model  "),
        device
            .model
            .as_deref()
            .or(client.info.model.as_deref())
            .unwrap_or("-"),
        ctx.out.dim("tvOS   "),
        client.info.os_version.as_deref().unwrap_or("-"),
        ctx.out.dim("address"),
        device.address,
    );
    if power == PowerState::Unknown {
        text.push_str(&format!(
            "\n\n  {}",
            ctx.out
                .yellow("The Apple TV did not report a power state; it may be asleep.")
        ));
    }
    ctx.out.text(text);
    Ok(())
}

const KNOWN_EVENTS: [&str; 5] = [
    "SystemStatus",
    "TVSystemStatus",
    "_iMC",
    "_tiStarted",
    "_tiStopped",
];

async fn watch(
    ctx: &mut Context,
    duration: Option<f64>,
    events: Option<Vec<String>>,
) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    let names = events.unwrap_or_else(|| KNOWN_EVENTS.iter().map(|s| s.to_string()).collect());
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    client.subscribe(&borrowed).await?;

    ctx.out.stream(json!({
        "type": "ready",
        "device": device.name,
        "events": names,
    }));

    let mut receiver = client.events();
    let deadline =
        duration.map(|secs| tokio::time::Instant::now() + Duration::from_secs_f64(secs.max(0.1)));
    loop {
        let received = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, receiver.recv()).await,
            None => Ok(receiver.recv().await),
        };
        match received {
            Ok(Ok(event)) => ctx.out.stream(translate(&event)),
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    Ok(())
}

fn translate(event: &clicker::companion::client::Event) -> Value {
    match event.name.as_str() {
        "SystemStatus" | "TVSystemStatus" => {
            let state = event
                .content
                .get("state")
                .and_then(|value| value.as_i64())
                .map(PowerState::from_attention_state)
                .unwrap_or(PowerState::Unknown);
            json!({ "type": "power", "state": state.as_str() })
        }
        "_iMC" => {
            let flags = event.content.get("_mcF").and_then(|value| value.as_i64());
            json!({
                "type": "media_control",
                "flags": flags,
                "volume_control": flags.map(|f| f & 0x0100 != 0),
                "play": flags.map(|f| f & 0x0001 != 0),
            })
        }
        "_tiStarted" => json!({
            "type": "text_focus",
            "focused": event.content.get("_tiD").is_some(),
        }),
        "_tiStopped" => json!({ "type": "text_focus", "focused": false }),
        other => json!({
            "type": "event",
            "name": other,
            "content": event.content.to_json(),
        }),
    }
}

async fn raw(
    ctx: &mut Context,
    identifier: String,
    content: Option<String>,
    event: bool,
) -> Result<()> {
    let content = match content {
        Some(raw) => {
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|err| Error::Usage(format!("--content is not valid JSON: {err}")))?;
            clicker::opack::Value::from_json(&parsed)?
        }
        None => clicker::opack::Value::Dict(Vec::new()),
    };
    let (device, client) = ctx.connect().await?;
    if event {
        client.send_event(&identifier, content).await?;
        ctx.out
            .ok(json!({ "device": device.name, "sent": identifier }));
        ctx.out.text(format!("sent event {identifier}"));
    } else {
        let response = client.raw(&identifier, content).await?;
        let payload = response
            .get("_c")
            .cloned()
            .unwrap_or(clicker::opack::Value::Null);
        ctx.out.ok(json!({
            "device": device.name,
            "identifier": identifier,
            "content": payload.to_json(),
            "response": response.to_json(),
        }));
        ctx.out.text(payload.to_string());
    }
    Ok(())
}

async fn doctor(ctx: &mut Context) -> Result<()> {
    let mut checks = Vec::new();

    let store_ok = ctx.store.save();
    checks.push(json!({
        "name": "credentials",
        "ok": store_ok.is_ok(),
        "detail": match &store_ok {
            Ok(()) => ctx.store.path().display().to_string(),
            Err(err) => err.to_string(),
        },
        "fix": "Check that your home directory is writable and retry.",
        "paired": ctx.store.devices().len(),
    }));

    let discovered = discovery::scan(Duration::from_secs(2), false).await;
    checks.push(json!({
        "name": "discovery",
        "ok": discovered.as_ref().map(|d| !d.is_empty()).unwrap_or(false),
        "detail": match &discovered {
            Ok(devices) => format!("{} candidate device(s) on this network", devices.len()),
            Err(err) => err.to_string(),
        },
        "fix": "Start mDNS (sudo systemctl enable --now avahi-daemon), wake the Apple TV, and check that AirPlay is On in Settings → AirPlay and Apple Home.",
    }));

    for device in ctx.store.devices().to_vec() {
        let credentials = match device.credentials() {
            Ok(credentials) => credentials,
            Err(err) => {
                checks.push(json!({
                    "name": format!("device:{}", device.name),
                    "ok": false,
                    "detail": err.to_string(),
                }));
                continue;
            }
        };
        match Client::connect(&device.address, device.port, &credentials, ctx.timeout).await {
            Ok(client) => checks.push(json!({
                "name": format!("device:{}", device.name),
                "ok": true,
                "detail": format!(
                    "connected at {}:{} (tvOS {})",
                    device.address,
                    device.port,
                    client.info.os_version.as_deref().unwrap_or("?")
                ),
            })),
            Err(err) => checks.push(json!({
                "name": format!("device:{}", device.name),
                "ok": false,
                "detail": err.to_string(),
                "fix": "Wake the Apple TV; if it still fails, run `clicker setup --force`.",
            })),
        }
    }

    let passed = checks
        .iter()
        .filter(|check| check.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .count();
    ctx.out.ok(json!({
        "checks": checks,
        "passed": passed,
        "total": checks.len(),
    }));
    let rows: Vec<Vec<String>> = checks
        .iter()
        .map(|check| {
            let ok = check.get("ok").and_then(Value::as_bool).unwrap_or(false);
            vec![
                if ok {
                    ctx.out.green("ok")
                } else {
                    ctx.out.red("fail")
                },
                check
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                check
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    ctx.out.text(ctx.out.table(&["", "CHECK", "DETAIL"], &rows));
    if ctx.store.devices().is_empty() {
        ctx.out.text(format!(
            "\n{}",
            ctx.out
                .dim("Nothing is paired yet. Run `clicker setup` to pair your Apple TV.")
        ));
    }

    for check in checks
        .iter()
        .filter(|check| !check.get("ok").and_then(Value::as_bool).unwrap_or(false))
    {
        if let Some(fix) = check.get("fix").and_then(Value::as_str) {
            ctx.out
                .text(format!("\n{} {}", ctx.out.yellow("next:"), fix));
        }
    }
    Ok(())
}

async fn batch(ctx: &mut Context) -> Result<()> {
    let (device, client) = ctx.connect().await?;
    ctx.out
        .stream(json!({ "type": "ready", "device": device.name }));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parsed: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(err) => {
                ctx.out.stream(
                    json!({ "type": "error", "code": "usage", "message": err.to_string() }),
                );
                continue;
            }
        };
        if let Err(err) = run_batch_op(&client, &parsed).await.map(|result| {
            ctx.out.stream(result);
        }) {
            ctx.out.stream(json!({
                "type": "error",
                "code": err.code(),
                "message": err.to_string(),
            }));
        }
    }
    Ok(())
}

async fn run_batch_op(client: &Client, op: &Value) -> Result<Value> {
    let kind = op
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Usage("batch op needs an \"op\" field".into()))?;
    match kind {
        "press" => {
            let action = op.get("action").and_then(Value::as_str).unwrap_or("single");
            let action = InputAction::parse(action)
                .ok_or_else(|| Error::Usage(format!("unknown action \"{action}\"")))?;
            let hold =
                Duration::from_millis(op.get("hold_ms").and_then(Value::as_u64).unwrap_or(1000));
            let keys: Vec<String> = match op.get("keys") {
                Some(Value::Array(keys)) => keys
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                _ => op
                    .get("key")
                    .and_then(Value::as_str)
                    .map(|key| vec![key.to_string()])
                    .unwrap_or_default(),
            };
            for key in &keys {
                let command = HidCommand::parse(key).ok_or_else(|| unknown_button(key))?;
                client.press(command, action, hold).await?;
            }
            Ok(json!({ "type": "press", "keys": keys, "action": action_name(action) }))
        }
        "text" => {
            if let Some(value) = op.get("set").and_then(Value::as_str) {
                let text = client.text_set(value).await?;
                Ok(json!({ "type": "text", "text": text }))
            } else if let Some(value) = op.get("append").and_then(Value::as_str) {
                let text = client.text_append(value).await?;
                Ok(json!({ "type": "text", "text": text }))
            } else if op.get("clear").and_then(Value::as_bool).unwrap_or(false) {
                client.text_clear().await?;
                Ok(json!({ "type": "text", "text": "" }))
            } else {
                let text = client.text_get().await?;
                Ok(json!({ "type": "text", "text": text }))
            }
        }
        "volume" => {
            if let Some(percent) = op.get("set").and_then(Value::as_f64) {
                client.set_volume(percent).await?;
                Ok(json!({ "type": "volume", "level": percent }))
            } else {
                let level = client.volume().await?;
                Ok(json!({ "type": "volume", "level": level }))
            }
        }
        "power" => {
            let state = op.get("set").and_then(Value::as_str).unwrap_or("status");
            match state {
                "on" => {
                    client
                        .press(HidCommand::Wake, InputAction::Single, Duration::ZERO)
                        .await?;
                    Ok(json!({ "type": "power", "state": "on" }))
                }
                "off" => {
                    client
                        .press(HidCommand::Sleep, InputAction::Single, Duration::ZERO)
                        .await?;
                    Ok(json!({ "type": "power", "state": "off" }))
                }
                _ => Ok(json!({
                    "type": "power",
                    "state": client.power_state().await.as_str(),
                })),
            }
        }
        "apps" => {
            let apps = client.app_list().await?;
            Ok(json!({
                "type": "apps",
                "apps": apps
                    .iter()
                    .map(|(bundle_id, name)| json!({ "bundle_id": bundle_id, "name": name }))
                    .collect::<Vec<_>>(),
            }))
        }
        "app" => {
            let target = op
                .get("launch")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Usage("app op needs \"launch\"".into()))?;
            client.launch_app(target).await?;
            Ok(json!({ "type": "app", "launched": target }))
        }
        "raw" => {
            let identifier = op
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Usage("raw op needs \"id\"".into()))?;
            let content = match op.get("content") {
                Some(value) => clicker::opack::Value::from_json(value)?,
                None => clicker::opack::Value::Dict(Vec::new()),
            };
            let response = client.raw(identifier, content).await?;
            Ok(json!({ "type": "raw", "id": identifier, "response": response.to_json() }))
        }
        "sleep" => {
            let ms = op.get("ms").and_then(Value::as_u64).unwrap_or(100);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(json!({ "type": "sleep", "ms": ms }))
        }
        other => Err(Error::Usage(format!("unknown batch op \"{other}\""))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_nearest_button() {
        let names: Vec<&str> = HidCommand::ALL
            .iter()
            .map(|command| command.name())
            .collect();
        assert_eq!(closest("dow", &names), Some("down"));
        assert_eq!(closest("selet", &names), Some("select"));
        assert_eq!(closest("hom", &names), Some("home"));
        assert_eq!(closest("volum-up", &names), Some("volume-up"));
        assert_eq!(closest("zzz", &names), None);
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("down", "down"), 0);
        assert_eq!(distance("dow", "down"), 1);
        assert_eq!(distance("dune", "down"), 3);
    }
}
