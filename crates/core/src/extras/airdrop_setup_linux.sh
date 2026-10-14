#!/bin/bash
# OpenHop: set up AirDrop on Linux (OpenHop opens this in a terminal).
# Installs omdrop-owl (AirDrop's Wi-Fi link in software, github.com/t4t5/omdrop-owl)
# and omdrop (sending and receiving, github.com/netmojo-systems/omdrop-plugin).
#   $1  the Wi-Fi interface for AirDrop ("" = omdrop-owl picks)
#   $2  "untested" when that card isn't on omdrop-owl's tested list
set -euo pipefail
iface=${1:-} untested=${2:-}
say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
done_() { printf '\n%s\n' "$*"; read -rp "Press Enter to close. "; exit "${3:-0}"; }
trap 'printf "\n\033[1;31mSet-up stopped (see above).\033[0m\n"; read -rp "Press Enter to close. "' ERR
printf 'OpenHop: setting up AirDrop. It asks for your password to install.\n'

work="${XDG_CACHE_HOME:-$HOME/.cache}/openhop/airdrop"
mkdir -p "$work"

if ! command -v omdrop-owl >/dev/null 2>&1 && [ ! -x /usr/lib/omdrop/omdrop-discoverable ]; then
  say "1/3  omdrop-owl (AirDrop's Wi-Fi link)"
  src="$work/omdrop-owl"
  rm -rf "$src"
  if command -v apt-get >/dev/null; then
    sudo apt-get update -q
    sudo apt-get install -y -q build-essential cmake git pkg-config dpkg-dev libev-dev libpcap-dev \
      libnl-3-dev libnl-genl-3-dev libnl-route-3-dev python3-libarchive-c curl
    git clone -q --depth 1 https://github.com/t4t5/omdrop-owl "$src"
    (cd "$src" && tools/build-deb.sh)
    sudo apt-get install -y -q "$src"/omdrop-owl_*.deb
  elif command -v pacman >/dev/null; then
    if command -v yay >/dev/null; then yay -S --noconfirm omdrop-owl
    elif command -v paru >/dev/null; then paru -S --noconfirm omdrop-owl
    else
      sudo pacman -S --needed --noconfirm base-devel git
      git clone -q --depth 1 https://github.com/t4t5/omdrop-owl "$src"
      (cd "$src" && makepkg -si --noconfirm)
    fi
  elif command -v dnf >/dev/null; then
    sudo dnf install -y gcc gcc-c++ make cmake git pkgconf rpm-build libev-devel libpcap-devel libnl3-devel python3-libarchive-c curl
    git clone -q --depth 1 https://github.com/t4t5/omdrop-owl "$src"
    (cd "$src" && tools/build-rpm.sh)
    sudo dnf install -y "$src"/omdrop-owl-*.rpm
  else
    done_ "This Linux isn't one omdrop-owl packages for yet (Debian, Ubuntu, Arch, Fedora). See https://github.com/t4t5/omdrop-owl" "" 2
  fi
else
  say "1/3  omdrop-owl is already installed"
fi

say "2/3  omdrop (sending and receiving)"
plugin="$HOME/.config/omarchy/plugins/netmojo.omdrop"
if [ ! -d "$plugin/.git" ]; then
  mkdir -p "$(dirname "$plugin")"
  git clone -q --depth 1 https://github.com/netmojo-systems/omdrop-plugin "$plugin" \
    || git clone -q --depth 1 https://github.com/brentkearney/omdrop-plugin "$plugin"
else
  git -C "$plugin" pull -q --ff-only || true
fi
"$plugin/bin/omdrop" setup || true
mkdir -p "$HOME/.local/bin"
[ -x "$HOME/.local/bin/omdrop" ] || ln -sf "$plugin/bin/omdrop" "$HOME/.local/bin/omdrop"

say "3/3  The Wi-Fi card"
sudo mkdir -p /etc/omdrop
if [ -n "$iface" ]; then
  # A USB adapter just for AirDrop: the computer's own Wi-Fi stays as it is.
  echo "$iface" | sudo tee /etc/omdrop/infra-iface >/dev/null
  echo "AirDrop uses $iface."
fi
if [ "$untested" = untested ]; then
  sudo touch /etc/omdrop/allow-untested
  echo "This card isn't on omdrop-owl's tested list yet: it may or may not work."
fi
"$HOME/.local/bin/omdrop" doctor || true
# OpenHop decides when to be visible: all the time only on an adapter just
# for AirDrop (or a card that stays on Wi-Fi); otherwise 10 minutes at a time.
trap - ERR
done_ "AirDrop is set up. OpenHop makes this computer visible to iPhones and Macs (set to Everyone); drop files on the island's AirDrop to send."
