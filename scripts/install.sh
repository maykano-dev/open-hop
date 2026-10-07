#!/usr/bin/env bash
# OpenHop installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/maykano-dev/open-hop/main/scripts/install.sh | bash
#
# Downloads the latest release from GitHub and installs it the native way:
#   macOS  -> /Applications/OpenHop.app (from the .dmg)
#   Debian / Ubuntu / Mint / Pop!_OS -> .deb via apt
#   Fedora / RHEL / openSUSE         -> .rpm via dnf / zypper
#   anything else                    -> AppImage in ~/.local/bin
set -euo pipefail

REPO="maykano-dev/open-hop"
API="${OPENHOP_API:-https://api.github.com/repos/$REPO/releases/latest}"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
info() { printf '  \033[34m›\033[0m %s\n' "$*" >&2; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
die()  { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not installed."; }
need curl

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  if command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi
fi

bold "Installing OpenHop"

info "Looking up the latest release…"
JSON="$(curl -fsSL -H 'Accept: application/vnd.github+json' "$API")" \
  || die "Couldn't reach GitHub. Check your internet connection, or download manually from https://github.com/$REPO/releases"
TAG="$(printf '%s' "$JSON" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
[ -n "$TAG" ] || die "No published release found yet at https://github.com/$REPO/releases"
ok "Latest version: $TAG"

asset_url() {
  # First browser_download_url whose file name matches the given regex.
  printf '%s' "$JSON" | grep -o '"browser_download_url": *"[^"]*"' | sed -E 's/.*"([^"]+)"$/\1/' | grep -E "$1" | head -n1
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download() {
  local url="$1" out="$TMP/$(basename "$1")"
  info "Downloading $(basename "$url")…"
  curl -fL --progress-bar "$url" -o "$out" || die "Download failed."
  printf '%s' "$out"
}

OS="$(uname -s)"
ARCH="$(uname -m)"

# Close any running OpenHop so the new version can start cleanly.
if pgrep -x openhop-app >/dev/null 2>&1; then
  info "Closing the running OpenHop…"
  pkill -x openhop-app 2>/dev/null || true
  sleep 1
fi

if [ "$OS" = "Darwin" ]; then
  URL="$(asset_url '\.dmg$')"
  [ -n "$URL" ] || die "No macOS build in release $TAG."
  DMG="$(download "$URL")"
  info "Installing to /Applications…"
  MNT="$(mktemp -d)"
  hdiutil attach -nobrowse -quiet -mountpoint "$MNT" "$DMG"
  rm -rf /Applications/OpenHop.app 2>/dev/null || $SUDO rm -rf /Applications/OpenHop.app
  cp -R "$MNT/OpenHop.app" /Applications/ 2>/dev/null || $SUDO cp -R "$MNT/OpenHop.app" /Applications/
  hdiutil detach -quiet "$MNT"
  # Builds aren't notarised yet: clear the quarantine flag so it opens normally.
  xattr -cr /Applications/OpenHop.app 2>/dev/null || $SUDO xattr -cr /Applications/OpenHop.app || true
  ok "Installed /Applications/OpenHop.app"
  open /Applications/OpenHop.app || true
  echo
  bold "One more step on macOS"
  echo "  Allow OpenHop in System Settings › Privacy & Security › Accessibility"
  echo "  (and Input Monitoring if this Mac shares its keyboard and mouse), then reopen OpenHop."
  exit 0
fi

[ "$OS" = "Linux" ] || die "Unsupported OS: $OS. On Windows, run in PowerShell:  irm https://raw.githubusercontent.com/$REPO/main/scripts/install.ps1 | iex"
case "$ARCH" in
  x86_64|amd64) ;;
  *) die "Prebuilt Linux packages are x86_64 only (this machine is $ARCH). Build from source: https://github.com/$REPO#build-from-source" ;;
esac

setup_uinput() {
  # Lets OpenHop control this desktop under Wayland (the .deb/.rpm do this themselves).
  info "Enabling virtual input (uinput)…"
  $SUDO modprobe uinput 2>/dev/null || true
  echo uinput | $SUDO tee /etc/modules-load.d/openhop-uinput.conf >/dev/null
  echo 'KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput", TAG+="uaccess"' \
    | $SUDO tee /etc/udev/rules.d/60-openhop-uinput.rules >/dev/null
  $SUDO udevadm control --reload-rules 2>/dev/null || true
  $SUDO udevadm trigger --name-match=uinput 2>/dev/null || true
  $SUDO usermod -aG input "${SUDO_USER:-$USER}" 2>/dev/null || true
}

if command -v apt-get >/dev/null 2>&1; then
  URL="$(asset_url '\.deb$')"; [ -n "$URL" ] || die "No .deb in release $TAG."
  PKG="$(download "$URL")"
  info "Installing with apt (you may be asked for your password)…"
  $SUDO apt-get install -y "$PKG"
elif command -v dnf >/dev/null 2>&1 || command -v zypper >/dev/null 2>&1; then
  URL="$(asset_url '\.rpm$')"; [ -n "$URL" ] || die "No .rpm in release $TAG."
  PKG="$(download "$URL")"
  info "Installing the .rpm (you may be asked for your password)…"
  if command -v dnf >/dev/null 2>&1; then $SUDO dnf install -y "$PKG"; else $SUDO zypper --non-interactive install --allow-unsigned-rpm "$PKG"; fi
  setup_uinput
else
  URL="$(asset_url '\.AppImage$')"; [ -n "$URL" ] || die "No AppImage in release $TAG."
  APP="$(download "$URL")"
  mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
  install -m 755 "$APP" "$HOME/.local/bin/OpenHop.AppImage"
  cat > "$HOME/.local/share/applications/openhop.desktop" <<EOF
[Desktop Entry]
Name=OpenHop
Comment=One keyboard and mouse for all your computers
Exec=$HOME/.local/bin/OpenHop.AppImage
Terminal=false
Type=Application
Categories=Utility;
EOF
  setup_uinput
fi

ok "OpenHop $TAG installed"
echo
bold "Next"
echo "  Open OpenHop from your applications menu, set the same passphrase on every computer,"
echo "  and pick 'Control Others' on the one whose keyboard and mouse you use."
echo "  If this is the first install, log out and back in once so input permissions apply."
command -v ufw >/dev/null 2>&1 && $SUDO ufw status 2>/dev/null | grep -q "Status: active" && {
  info "ufw firewall is active: opening OpenHop's ports"
  $SUDO ufw allow 24850:24890/tcp >/dev/null && $SUDO ufw allow 24851/udp >/dev/null
}
exit 0
