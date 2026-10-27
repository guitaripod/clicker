# clicker — notes for agents

Apple TV remote over Apple's Companion Link protocol. Rust, single binary, no
runtime dependencies beyond a working mDNS resolver.

## Build and verify

```sh
cargo test                       # opack, tlv8, srp vectors, crypto, text archives
cargo clippy --all-targets       # must stay clean
scripts/install.sh               # installs to ~/.cargo/bin and prints the version
```

Anything that touches the wire must be checked against real hardware:

```sh
clicker scan                     # a real Apple TV shows with airplay_confirmed: true
clicker status --json
clicker press home && clicker press home
```

## Rules that keep the protocol working

- OPACK replies reference earlier objects by index (`0xA0`-`0xC4`). Never encode
  a message by hand-rolling bytes; go through `opack::pack`. Decoding must keep
  appending to the object list for every non-scalar, in wire order.
- The SRP client must stay byte-identical to srptools: `k = H(N||PAD(g))`,
  `M1 = H(H(N)^H(g) || H(I) || s || A || B || K)` with minimally encoded big
  endian integers. `src/srp.rs` has the pinned python vectors; if they fail,
  pairing fails with error 0x02 and the TV says the PIN was wrong.
- Encrypted frames use a little-endian counter nonce and the 4-byte header as
  AAD. Pairing TLVs use ASCII nonces (`PS-Msg05`, `PV-Msg02`) zero-padded to 12
  bytes. Never reuse a counter across directions.
- Connect order matters: `_systemInfo`, `_touchStart`, `_sessionStart`,
  `TVRCSessionStart`, `_tiStart`, then `_interest`. tvOS 26.5+ silently ignores
  `FetchLaunchableApplicationsEvent` and `FetchAttentionState` without the TV
  Remote Client session.
- Pairing is destructive to test: each attempt makes the TV show a new PIN, and
  repeated failures rate-limit. Ask for the PIN instead of guessing.

## Onboarding

`clicker setup` is the front door and the only path a non-technical user is
expected to take. It must stay three numbered steps, narrate in plain language,
never print raw protocol errors at a human, and always end with the next command
to try. Every failure it can hit has a remediation list (`setup::onboard_error`),
and the same failure must stay readable as JSON: `{"ok":false,"error":{...,
"fixes":[...]}}`. Never leave a user at a bare error.

`clicker guide` is the cheat sheet; when you add a command, add it there and to
the README table in the same commit.

## Conventions

- No inline comments; explain with a named private function plus a doc comment.
- Human output is styled only on a terminal (`Output::paint` respects NO_COLOR
  and never colours JSON). Progress narration uses `Output::note` (stderr) or
  `Output::event` (JSON-only), never `println!`.
- `--json` is a contract: one object, `ok` at the top level, stable error codes
  and exit codes (`src/error.rs`). Do not add commands that break it.
- Credentials live in `~/.config/clicker/devices.json` (0600) in pyatv's
  four-field format. Never print them.
- A wrong PIN is not retryable: the Apple TV answers HAP error 0x03 (backoff)
  immediately after one failure. Surface the wait, do not loop harder.
