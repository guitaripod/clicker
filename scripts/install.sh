#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '%s\n' "$*"; }
os="$(uname -s)"

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

install_completions() {
  case "$os" in
    Darwin)
      if [ -d "$HOME/.zfunc" ]; then
        "$binary" completions zsh >"$HOME/.zfunc/_clicker" 2>/dev/null || true
      fi
      ;;
    *)
      local dir="$HOME/.local/share/bash-completion/completions"
      if [ -d "$(dirname "$dir")" ] || mkdir -p "$dir" 2>/dev/null; then
        "$binary" completions bash >"$dir/clicker" 2>/dev/null || true
      fi
      ;;
  esac
}
install_completions

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

case "$os" in
  Darwin)
    if ! pgrep -x mDNSResponder >/dev/null 2>&1; then
      say ""
      say "Heads up: mDNSResponder is not running, and finding the Apple TV needs it."
      say "  sudo launchctl kickstart -k system/com.apple.mDNSResponder"
    fi
    say ""
    say "If discovery finds nothing, allow Local Network access for your terminal in"
    say "System Settings → Privacy & Security → Local Network."
    ;;
  *)
    if command -v systemctl >/dev/null 2>&1 && ! systemctl is-active --quiet avahi-daemon 2>/dev/null; then
      say ""
      say "Heads up: avahi-daemon is not running, and finding the Apple TV needs it."
      say "  sudo systemctl enable --now avahi-daemon"
    fi
    ;;
esac

say ""
say "Next, pair your Apple TV (about a minute):"
say "  clicker setup"
