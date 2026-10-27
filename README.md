# clicker

[![CI](https://github.com/guitaripod/clicker/actions/workflows/ci.yml/badge.svg)](https://github.com/guitaripod/clicker/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)

An Apple TV remote for the terminal, written in Rust.

No app on the TV, no phone, no account, no cloud: `clicker` talks to the Apple TV
over your own network using the same protocol as the Remote widget in Control
Center. One small binary, works on Linux, and every command can print JSON for
scripts and agents.

```
$ clicker setup
1/3  Looking for your Apple TV
     Found Apple TV (Apple TV 4K, 3rd generation, tvOS 18.6).
2/3  Pairing
     Apple TV will show a 4-digit code. Type it here, on this line.
     4-digit code on the TV: 4821
     ok paired as "Clicker".
3/3  Checking the connection
     ok Connected to tvOS 18.6.0, power on, volume 30%.

Done. clicker is ready.

Try this next:
  clicker press home      go to the home screen
  clicker status          power, volume and tvOS version
  clicker tui             use the whole remote on screen
  clicker guide           the full cheat sheet
```

## Install

Rust 1.87 or newer. On Linux you also need Avahi (or another mDNS responder),
which most desktops already run; macOS has mDNSResponder built in.

```sh
git clone https://github.com/guitaripod/clicker.git
cd clicker
scripts/install.sh
```

The script checks for Rust, builds the binary, installs it to `~/.cargo/bin`,
adds shell completions, and warns if mDNS discovery is not running. If Rust is
missing it prints the one command that installs it.

Already have Rust? Then this is the same thing with no clone:

```sh
cargo install --git https://github.com/guitaripod/clicker --locked
```

Or take a prebuilt binary from the [latest
release](https://github.com/guitaripod/clicker/releases/latest) — Linux x86_64
and macOS arm64, with SHA-256 checksums — and put it somewhere on your `PATH`:

```sh
tar xzf clicker-*-x86_64-unknown-linux-gnu.tar.gz
sudo install -m755 clicker /usr/local/bin/clicker
```

The macOS tarball is ad-hoc signed, so if Gatekeeper refuses a downloaded
copy, clear the quarantine flag once: `xattr -d com.apple.quarantine clicker`.

**macOS:** works the same way, including on Apple Silicon. If discovery finds
nothing, allow Local Network access for your terminal in System Settings →
Privacy & Security → Local Network (a CLI started from a terminal inherits the
terminal's permission).

Tested on Arch Linux and macOS 27; CI builds and tests both.

## Set up your Apple TV (about a minute)

```sh
clicker setup
```

1. It looks for Apple TVs on your network. If there are several, it asks which
   one — type the number and press enter.
2. Your TV shows a 4-digit code. Type it on the computer and press enter. The
   code is different every time, so read it from the screen when asked.
3. It connects once to prove the pairing works, then prints a few commands to
   try.

That is the whole setup. The pairing is stored in
`~/.config/clicker/devices.json` on Linux and
`~/Library/Application Support/clicker/devices.json` on macOS (readable only by
you, mode 0600) and survives reboots.

Prefer to be told what to do? `clicker guide` prints every command, grouped by
task. `clicker doctor` checks the parts that can break.

## Everyday use

```sh
clicker press home                 # home screen
clicker press down down select     # navigate and pick
clicker press menu                 # back
clicker volume up                  # one step
clicker power off                  # sleep
clicker tui                        # the whole remote, on screen
```

Type into a search field when the TV keyboard is up:

```sh
clicker apps launch com.apple.TVAppStore
clicker text set "blade runner"
```

Send several buttons without paying for a new connection each time:

```sh
printf '%s\n' '{"op":"press","keys":["down","down","select"]}' | clicker batch --json
```

## The remote, in Vim

`clicker tui` is the whole Siri Remote on screen, and it speaks Vim: counts,
macros, `.` and a `:` command line with tab completion.

| Keys | What it does |
| --- | --- |
| `h` `j` `k` `l` | press left, down, up, right (counts work: `5j`) |
| `J` `K` | hold down, hold up (fast scrolling) |
| `enter` | select |
| `m` | back |
| `gg` or `H` | home |
| `p` `s` `c` `r` | power, Siri, clickpad, refresh |
| `>` `<` or `ctrl-a` `ctrl-x` | volume up, down |
| `w` `b` | skip ±10 seconds |
| `ctrl-d` `ctrl-u` | page down, page up |
| `[` `]` | channel down, channel up |
| `i` `a` | type text, or append to it |
| `o` or `/` | launch an app, with fuzzy search and recents |
| `:` | command line (`:vol 40`, `:app net`, `:seek -15`, `:swipe up`) |
| `?` | every key, on one screen |
| `q a` … `q`, `@ a` | record a macro, then replay it (`@@` repeats the last one) |
| `.` | repeat the last change |
| `:q` or `Z Z` | quit |

Apps are fetched once per session, recents float to the top, and every command
reports what happened in the status line. If the Apple TV sleeps, wakes or
moves, the remote reconnects and refreshes itself instead of going dead. If the
window gets too small for the remote, it tells you the size it wants.

## What it can do

| Siri Remote | `clicker` |
| --- | --- |
| D-pad, select, back, TV | `press up`, `press select`, `press menu`, `press home` |
| Touch surface, swipes, clickpad | `touch swipe`, `touch tap`, `touch hold`, `touch click` |
| Play/pause, skip, next/previous | `press play-pause`, `media skip 10`, `media next` |
| Volume | `volume up`, `volume get`, `volume set 30` |
| Power on/off | `power on`, `power off`, `power status` |
| Typing into search fields | `text set`, `text append`, `text clear`, `text get` |
| App launcher | `apps list`, `apps launch <bundle id or url>` |
| User switching | `accounts list`, `accounts switch <id>` |
| — | `status`, `watch`, `raw`, `doctor`, `batch`, `tui`, `guide` |

Siri voice input is the one thing a network remote cannot do: the remote streams
its microphone to the Apple TV over its own encrypted Bluetooth link, and this
protocol has no way to inject audio.

## If something goes wrong

| What you see | What to do |
| --- | --- |
| `no Apple TV answered on this network` | Check the TV and the computer are on the same network. On the TV: Settings → AirPlay and Apple Home → AirPlay **On**. Wake the TV with its remote and try again. |
| A second or third device in the list | Macs with Continuity advertise an Apple TV model too. They are shown with a leading `~`. Pair the real one with `clicker setup --device "Apple TV"`. |
| `the Apple TV did not show a code` | The TV was asleep or off. Wake it with the Siri Remote and run `clicker setup` again. |
| `the Apple TV rejected the code` | The code changes every attempt. Watch the TV while typing; `clicker setup` retries twice before giving up. |
| `paired, but the connection check failed` | The TV went back to sleep. Wake it, run `clicker status`; if it still fails, `clicker setup --force`. |
| macOS: `no Apple TV answered` | Allow Local Network access for your terminal in System Settings → Privacy & Security → Local Network, then retry. Right after waking the Apple TV its mDNS can be silent for a minute; `clicker setup` now keeps looking for about half a minute. |
| Anything else | `clicker doctor` prints each check with a fix, and `-vv` shows the protocol frames. |

## JSON contract

Every command takes `--json` (or `CLICKER_JSON=1`) and prints exactly one
object, with `ok` at the top level. Errors never go to stdout in text mode; in
JSON mode they arrive as
`{"ok":false,"error":{"code","message","hint","fixes","exit_code"}}`.

```console
$ clicker status --json
{"ok":true,"device":{"id":"a1b2c3d4…","name":"Apple TV","model":"AppleTV14,1",
 "address":"192.168.1.42","port":57905},"power":"on","volume":35,
 "media_control":{"flags":340,"volume_control":true,"play":true,"skip":true}}

$ clicker watch --json --duration 30
{"ok":true,"event":{"type":"ready","device":"Apple TV","events":["SystemStatus",…]}}
{"ok":true,"event":{"type":"power","state":"on"}}
{"ok":true,"event":{"type":"media_control","flags":340,"volume_control":true,"play":true}}
{"ok":true,"event":{"type":"text_focus","focused":true}}
```

`clicker setup --json` streams progress events (`searching`, `found`,
`selected`, `pin_required`, `paired`, `verified`, `done`) and reads the PIN from
stdin, so a script or an agent can drive the whole first run:

```console
$ clicker setup --json --device "Apple TV"
{"ok":true,"event":{"type":"pin_required","attempt":1,"prompt":"type the 4-digit code shown on the Apple TV"}}
```

Exit codes: `0` success, `2` usage, `3` no such device, `4` not paired,
`5` authentication failed, `6` timeout, `7` protocol error, `8` config or
discovery failure.

### Batching

One process per button press costs a TCP connection and a pair-verify handshake.
For sequences, `clicker batch` reads newline-delimited JSON on stdin and reuses
one session:

```console
$ printf '%s\n' \
    '{"op":"press","keys":["down","down","select"]}' \
    '{"op":"text","set":"blade runner"}' \
    '{"op":"sleep","ms":300}' \
    '{"op":"press","key":"select"}' \
  | clicker batch --json
{"ok":true,"event":{"type":"ready","device":"Apple TV"}}
{"ok":true,"event":{"type":"press","keys":["down","down","select"],"action":"single"}}
{"ok":true,"event":{"type":"text","text":"blade runner"}}
{"ok":true,"event":{"type":"sleep","ms":300}}
{"ok":true,"event":{"type":"press","keys":["select"],"action":"single"}}
```

Ops: `press` (`key` or `keys`, `action`, `hold_ms`), `text` (`set`, `append`,
`clear`, or nothing for get), `volume` (`set` or nothing for get), `power`
(`set`: `on`/`off`/`status`), `apps` (list), `app` (`launch`), `raw` (`id`,
`content`), `sleep` (`ms`).

## Commands

```
clicker setup [--pin N] [--name NAME] [--force]     guided first run
clicker guide                                      cheat sheet
clicker scan [--timeout 2] [--all]                 find Apple TVs over Bonjour
clicker pair [--pin N] [--name NAME]               pair without the hand-holding
clicker devices                                    list paired devices
clicker forget <device>                            drop credentials
clicker press <keys…> [--action single|double|hold] [--hold-ms N] [--repeat N]
clicker media play|pause|play-pause|next|previous|skip <secs>|fast-forward|rewind
clicker volume get|set <pct>|up|down
clicker power on|off|status
clicker apps list|launch <bundle-id|url>
clicker accounts list|switch <id>
clicker text set|append|clear|get
clicker touch swipe --from X,Y --to X,Y [--duration-ms N]
clicker touch tap|hold X,Y [--duration-ms N]
clicker touch click [--double]
clicker status                                     power, volume and capabilities
clicker watch [--duration S] [--events …]          stream NDJSON events
clicker raw <identifier> [--content JSON] [--event]
clicker batch                                      NDJSON commands on stdin
clicker doctor                                     check discovery, credentials, connections
clicker tui                                        interactive remote with Vim keys
clicker keys                                       list button names
clicker completions <shell>
```

Global flags: `--device <id|name|address>` (env `CLICKER_DEVICE`), `--json`
(env `CLICKER_JSON`), `--timeout <secs>`, `-v`/`-vv`.

If a device's address changes, `clicker` rediscovers it by ID, updates the
store, and retries once.

## Buttons

`up down left right select menu home volume-up volume-down siri screensaver
sleep wake play-pause channel-up channel-down guide page-up page-down`

Aliases: `enter`/`ok` → select, `back` → menu, `tv` → home, `control-center` →
page-down.

## Layout

```
src/opack.rs            OPACK serialisation with the object-reference table
src/tlv8.rs             HAP TLV8
src/srp.rs              SRP-6a / SHA-512 (3072-bit group), with srptools vectors
src/crypto.rs           ChaCha20-Poly1305 framing, HKDF-SHA512
src/hap.rs              pair-setup (PIN) and pair-verify, both signatures checked
src/companion/          frames, transport, session, commands, events
src/text.rs             NSKeyedArchiver payloads for Remote Text Input
src/discovery.rs        Bonjour, joining Companion and AirPlay records
src/store.rs            credentials, 0600
src/app.rs              shared command context and reconnect logic
src/setup.rs            guided onboarding
src/guide.rs            the cheat sheet
src/main.rs             CLI
src/tui/                on-screen remote: Vim grammar, palette, rendering
```

Everything above is reverse engineered from pyatv and itsytv-core, whose
protocol notes and tests this project follows. If a future tvOS breaks a
command, `clicker doctor` and `clicker raw` are the levers.

## Not implemented

- Siri microphone audio (impossible over this protocol).
- Now-playing metadata and artwork (that is MRP over an AirPlay session; the
  physical remote does not have it either).
- AirPlay streaming.

## License

GPL-3.0-or-later.
