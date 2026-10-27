//! `clicker setup`: guided onboarding that assumes no prior knowledge.

use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::Context;
use crate::companion::client::Client;
use crate::companion::conn::Connection;
use crate::discovery::{self, Discovered};
use crate::guide;
use crate::hap::PairSetup;
use crate::store::{PairedDevice, Store};
use crate::{Error, Result};

const MAX_PIN_ATTEMPTS: u8 = 3;

pub struct Options {
    pub pin: Option<String>,
    pub force: bool,
    pub name: String,
}

pub async fn run(ctx: &mut Context, options: Options) -> Result<()> {
    heading(ctx, "1/3", "Looking for your Apple TV…");
    ctx.out.event(json!({ "type": "searching" }));
    let found = discovery::scan(Duration::from_millis(2500), false)
        .await
        .map_err(|err| {
            onboard_error(
                ctx,
                "discovery_failed",
                8,
                &err.to_string(),
                &[
                    "On Arch and most desktops: sudo systemctl enable --now avahi-daemon",
                    "On a minimal server, install avahi (or any mDNS responder) first.",
                    "Then run `clicker setup` again.",
                ],
            )
        })?;
    let pairable: Vec<Discovered> = found.into_iter().filter(|device| device.pairable).collect();
    // Records that also have a matching AirPlay advertisement are real Apple
    // TVs; the rest are usually a Mac running the Remote service.
    let confirmed: Vec<Discovered> = pairable
        .iter()
        .filter(|device| device.airplay_confirmed)
        .cloned()
        .collect();
    let candidates = if confirmed.is_empty() {
        pairable
    } else {
        confirmed
    };

    if candidates.is_empty() {
        let fixes = [
            "Check that the Apple TV and this computer are on the same network (same Wi-Fi, or both wired).",
            "On the Apple TV go to Settings → AirPlay and Apple Home and make sure AirPlay is On.",
            "Wake the Apple TV with its remote, then run `clicker setup` again.",
            "Still nothing? `clicker doctor` shows what this machine can discover.",
        ];
        return Err(onboard_error(
            ctx,
            "no_devices",
            3,
            "no Apple TV answered on this network",
            &fixes,
        ));
    }

    ctx.out.event(json!({
        "type": "found",
        "devices": candidates.iter().map(device_json).collect::<Vec<_>>(),
    }));

    let target = match &ctx.selector {
        Some(selector) => match match_selector(&candidates, selector) {
            Some(device) => device,
            None => {
                let names = candidates
                    .iter()
                    .map(|device| device.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let fixes = [format!("Pass one of: {names}")];
                return Err(onboard_error(
                    ctx,
                    "device_not_found",
                    3,
                    &format!("\"{selector}\" is not on this network"),
                    &fixes,
                ));
            }
        },
        None => choose(ctx, &candidates)?,
    };

    if let Some(existing) = ctx
        .store
        .devices()
        .iter()
        .find(|device| device.id == target.id)
        .cloned()
    {
        if !options.force {
            heading(ctx, "2/3", "Checking the existing pairing");
            ctx.out.event(json!({
                "type": "already_paired",
                "device": Context::device_json(&existing),
            }));
            ctx.out.text(format!(
                "     {} is already set up as \"{}\".",
                ctx.out.bold(&existing.name),
                existing.display_name
            ));
            verify_and_finish(ctx, &existing).await?;
            return Ok(());
        }
        heading(ctx, "2/3", "Pairing again");
    } else {
        heading(ctx, "2/3", "Pairing");
        ctx.out.text(format!(
            "     {} will show a 4-digit code. Type it here, on this line.",
            ctx.out.bold(&target.name)
        ));
    }

    let credentials = pair_with_retries(ctx, &target, &options).await?;
    let device = Store::from_discovery(&target, credentials, &options.name);
    ctx.store.upsert(device.clone());
    ctx.store.save()?;

    ctx.out.event(json!({
        "type": "paired",
        "device": Context::device_json(&device),
        "display_name": device.display_name,
    }));
    ctx.out.text(format!(
        "     {} paired as \"{}\".",
        ctx.out.green("ok"),
        device.display_name
    ));

    verify_and_finish(ctx, &device).await
}

async fn pair_with_retries(
    ctx: &mut Context,
    target: &Discovered,
    options: &Options,
) -> Result<crate::hap::Credentials> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut conn = Connection::open(&target.address, target.port, ctx.timeout).await?;
        let setup = PairSetup::begin(&mut conn).await.map_err(|err| match err {
            Error::PairingBackoff { seconds } => backoff_failure(ctx, seconds),
            other => other,
        })?;

        let pin = match (&options.pin, attempt) {
            (Some(pin), _) => pin.clone(),
            (None, _) => match read_pin(ctx, attempt)? {
                Some(pin) => pin,
                None => {
                    let fixes = [
                        "The pairing session ended, so the code on the TV expired.",
                        "Run `clicker setup` again and type the new code.",
                    ];
                    return Err(onboard_error(
                        ctx,
                        "no_pin",
                        2,
                        "no code was entered",
                        &fixes,
                    ));
                }
            },
        };

        match setup.finish(&mut conn, &options.name, &pin).await {
            Ok(credentials) => return Ok(credentials),
            Err(Error::Authentication(message))
                if options.pin.is_none() && attempt < MAX_PIN_ATTEMPTS =>
            {
                ctx.out.event(json!({
                    "type": "pin_rejected",
                    "attempt": attempt,
                    "message": message,
                }));
                ctx.out.text(format!(
                    "     That code did not match. The TV shows a {} one — look at the screen again.",
                    ctx.out.bold("new")
                ));
            }
            Err(Error::PairingBackoff { seconds }) => return Err(backoff_failure(ctx, seconds)),
            Err(Error::Authentication(_)) if options.pin.is_none() => {
                let fixes = [
                    "The code changes on every attempt, so watch the TV while typing.",
                    "Make sure you are reading the code on the Apple TV, not another device.",
                ];
                return Err(onboard_error(
                    ctx,
                    "pin_rejected",
                    5,
                    "the Apple TV rejected the code three times",
                    &fixes,
                ));
            }
            Err(err) => return Err(err),
        }
    }
}

fn read_pin(ctx: &Context, attempt: u8) -> Result<Option<String>> {
    ctx.out.event(json!({
        "type": "pin_required",
        "attempt": attempt,
        "prompt": "type the 4-digit code shown on the Apple TV",
    }));
    if !ctx.out.json {
        eprint!("     {} ", ctx.out.cyan("4-digit code on the TV:"));
        std::io::stderr().flush().ok();
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line)?;
    if !ctx.out.json && !std::io::stdin().is_terminal() {
        eprintln!();
    }
    let pin = line.trim().to_string();
    if read == 0 || pin.is_empty() {
        return Ok(None);
    }
    Ok(Some(pin))
}

async fn verify_and_finish(ctx: &mut Context, device: &PairedDevice) -> Result<()> {
    heading(ctx, "3/3", "Checking the connection");
    let credentials = device.credentials()?;
    let client = Client::connect(&device.address, device.port, &credentials, ctx.timeout).await;
    match client {
        Ok(client) => {
            let power = client.power_state().await;
            let volume = client.volume().await.ok().flatten();
            ctx.out.event(json!({
                "type": "verified",
                "device": Context::device_json(device),
                "power": power.as_str(),
                "volume": volume,
                "os_version": client.info.os_version,
            }));
            ctx.out.text(format!(
                "     {} Connected to tvOS {}, power {}, volume {}.",
                ctx.out.green("ok"),
                client.info.os_version.as_deref().unwrap_or("?"),
                power.as_str(),
                match volume {
                    Some(level) => format!("{level:.0}%"),
                    None => "controlled by the TV".into(),
                }
            ));
            ctx.out.event(json!({ "type": "done" }));
            ctx.out.text("");
            ctx.out.text(format!(
                "{} clicker is ready. Credentials are in {}.",
                ctx.out.green("Done."),
                ctx.out.dim(ctx.store.path().display().to_string())
            ));
            ctx.out.text("");
            guide::next_steps(&ctx.out);
            Ok(())
        }
        Err(err) => {
            let fixes = [
                "The Apple TV may be asleep; wake it and run `clicker status`.",
                "If this keeps failing, run `clicker setup --force` to pair again.",
            ];
            Err(onboard_error(
                ctx,
                "verify_failed",
                err.exit_code(),
                &format!("paired, but the connection check failed: {err}"),
                &fixes,
            ))
        }
    }
}

fn choose(ctx: &Context, candidates: &[Discovered]) -> Result<Discovered> {
    if candidates.len() == 1 {
        let device = &candidates[0];
        ctx.out.event(json!({
            "type": "selected",
            "device": device_json(device),
        }));
        ctx.out.text(format!(
            "     Found {} ({}).",
            ctx.out.bold(&device.name),
            describe(device)
        ));
        return Ok(device.clone());
    }

    let names = candidates
        .iter()
        .map(|device| device.name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    if !std::io::stdin().is_terminal() {
        let fixes = [format!("Pass --device with one of: {names}")];
        return Err(onboard_error(
            ctx,
            "device_ambiguous",
            3,
            &format!("found {} Apple TVs ({names})", candidates.len()),
            &fixes,
        ));
    }

    ctx.out.text("     Found more than one:");
    for (index, device) in candidates.iter().enumerate() {
        ctx.out.text(format!(
            "       {}. {} ({})",
            index + 1,
            device.name,
            describe(device)
        ));
    }
    ctx.out.event(json!({
        "type": "choose",
        "devices": candidates.iter().map(device_json).collect::<Vec<_>>(),
    }));
    eprint!("     Which one? [1] ");
    std::io::stderr().flush().ok();

    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let choice = line.trim();
    let index = if choice.is_empty() {
        0
    } else {
        choice
            .parse::<usize>()
            .ok()
            .filter(|index| *index >= 1 && *index <= candidates.len())
            .map(|index| index - 1)
            .ok_or_else(|| {
                onboard_error(
                    ctx,
                    "invalid_choice",
                    2,
                    &format!("\"{choice}\" is not one of the listed numbers"),
                    &["Run `clicker setup` again and pick a number from the list."],
                )
            })?
    };
    let device = candidates[index].clone();
    ctx.out.event(json!({
        "type": "selected",
        "device": device_json(&device),
    }));
    Ok(device)
}

fn match_selector(candidates: &[Discovered], selector: &str) -> Option<Discovered> {
    let needle = selector.trim().to_ascii_lowercase();
    candidates
        .iter()
        .find(|device| {
            device.id.to_ascii_lowercase() == needle
                || device.id.to_ascii_lowercase().starts_with(&needle)
                || device.address == needle
                || device.name.to_ascii_lowercase() == needle
        })
        .or_else(|| {
            candidates
                .iter()
                .find(|device| device.name.to_ascii_lowercase().contains(&needle))
        })
        .cloned()
}

fn device_json(device: &Discovered) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "model": device.model,
        "os_version": device.os_version,
        "address": device.address,
        "port": device.port,
    })
}

fn describe(device: &Discovered) -> String {
    let model = match device.model.as_deref() {
        Some("AppleTV14,1") => "Apple TV 4K, 3rd generation",
        Some("AppleTV11,1") => "Apple TV 4K, 2nd generation",
        Some("AppleTV6,2") => "Apple TV 4K, 1st generation",
        Some("AppleTV5,3") => "Apple TV HD",
        Some(model) => model,
        None => "Apple TV",
    };
    match &device.os_version {
        Some(version) => format!("{model}, tvOS {version}"),
        None => model.to_string(),
    }
}

fn heading(ctx: &Context, step: &str, title: &str) {
    ctx.out
        .text(format!("{}  {}", ctx.out.bold(step), ctx.out.bold(title)));
}

fn backoff_failure(ctx: &Context, seconds: Option<u64>) -> Error {
    let wait = seconds.unwrap_or(60);
    let fixes = [
        format!("Wait about {wait} seconds, then run `clicker setup` again."),
        "The Apple TV shows a new code for every attempt, so read it just before typing."
            .to_string(),
    ];
    onboard_error(
        ctx,
        "pairing_backoff",
        5,
        "the Apple TV is limiting pairing attempts after a wrong code",
        &fixes,
    )
}

/// Report a setup failure with remediation steps and remember that it has been
/// printed, so the top-level handler only contributes the exit code.
pub fn onboard_error(
    ctx: &Context,
    code: &str,
    exit_code: i32,
    message: &str,
    fixes: &[impl AsRef<str>],
) -> Error {
    let fixes: Vec<String> = fixes.iter().map(|fix| fix.as_ref().to_string()).collect();
    ctx.out.fail(code, exit_code, message, &fixes);
    Error::Reported { exit_code }
}
