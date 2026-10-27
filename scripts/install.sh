#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '%s\n' "$*"; }

if ! command -v cargo >/dev/null 2>&1; then
  say "Rust is not installed yet."
  say ""
  say "Install it with:"
  say "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  say "  source \"\$HOME/.cargo/env\""
  say ""
  say "Then run this script again."
  exit 1
fi

say "Building clicker (this takes a minute the first time)…"
cargo install --path . --locked --force

binary="$HOME/.cargo/bin/clicker"
if [ ! -x "$binary" ]; then
  binary="$(command -v clicker)"
fi

completion_dir="$HOME/.local/share/bash-completion/completions"
if [ -d "$(dirname "$completion_dir")" ] || mkdir -p "$completion_dir" 2>/dev/null; then
  "$binary" completions bash >"$completion_dir/clicker" 2>/dev/null || true
fi

say ""
say "$("$binary" --version) installed at $binary"

case ":$PATH:" in
  *":$HOME/.cargo/bin:"*) ;;
  *)
    say ""
    say "One more step: add this to your shell profile (~/.bashrc or ~/.zshrc)"
    say "  export PATH=\"\$HOME/.cargo/bin:\$PATH\""
    ;;
esac

if command -v systemctl >/dev/null 2>&1 && ! systemctl is-active --quiet avahi-daemon 2>/dev/null; then
  say ""
  say "Heads up: avahi-daemon is not running, and finding the Apple TV needs it."
  say "  sudo systemctl enable --now avahi-daemon"
fi

say ""
say "Next, pair your Apple TV (about a minute):"
say "  clicker setup"
