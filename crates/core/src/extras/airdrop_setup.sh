#!/bin/bash
# OpenHop: AirDrop on Windows, the Linux half (run by airdrop_setup.ps1 in
# WSL). Modes:
#   root  FOLDER USER           packages, the kernel (to FOLDER/bzImage),
#                               the adapter's firmware, omdrop-owl, omdrop
#   user  FOLDER USER DOWNLOADS omdrop's own set-up, received files to DOWNLOADS
#   check FOLDER USER           is the adapter there, and omdrop ready?
set -euo pipefail
mode=$1 folder=$2 user=$3
say() { printf '\n-- %s\n' "$*"; }

case "$mode" in
root)
  export DEBIAN_FRONTEND=noninteractive
  say "Packages"
  apt-get update -q
  apt-get install -y -q build-essential flex bison libssl-dev libelf-dev bc dwarves cpio python3 git curl \
    cmake pkg-config dpkg-dev libev-dev libpcap-dev libnl-3-dev libnl-genl-3-dev libnl-route-3-dev \
    iw usbutils policykit-1 python3-libarchive-c avahi-daemon libnss-mdns || \
  apt-get install -y -q build-essential flex bison libssl-dev libelf-dev bc dwarves cpio python3 git curl \
    cmake pkg-config dpkg-dev libev-dev libpcap-dev libnl-3-dev libnl-genl-3-dev libnl-route-3-dev \
    iw usbutils polkitd python3-libarchive-c avahi-daemon libnss-mdns

  # systemd (omdrop's receiver is a user service).
  if ! grep -q '^systemd=true' /etc/wsl.conf 2>/dev/null; then
    printf '\n[boot]\nsystemd=true\n' >> /etc/wsl.conf
  fi

  if [ ! -f "$folder/bzImage" ]; then
    say "Linux kernel with Wi-Fi and the MT7612U driver (20-40 minutes)"
    ver=$(uname -r | cut -d- -f1)
    src=/usr/src/openhop-wsl-kernel
    rm -rf "$src"
    git clone -q --depth 1 --branch "linux-msft-wsl-$ver" https://github.com/microsoft/WSL2-Linux-Kernel "$src" \
      || git clone -q --depth 1 https://github.com/microsoft/WSL2-Linux-Kernel "$src"
    cd "$src"
    case "$(uname -m)" in
      aarch64) cfg=Microsoft/config-wsl-arm64; image=arch/arm64/boot/Image ;;
      *) cfg=Microsoft/config-wsl; image=arch/x86/boot/bzImage ;;
    esac
    cp "$cfg" .config
    for o in RFKILL WIRELESS WLAN CFG80211 CFG80211_WEXT MAC80211 MAC80211_RC_MINSTREL \
             WLAN_VENDOR_MEDIATEK MT76x2U USB USB_NET_DRIVERS USBIP_CORE USBIP_VHCI_HCD; do
      scripts/config --enable "$o"
    done
    scripts/config --set-str LOCALVERSION "-openhop"
    make olddefconfig >/dev/null
    grep -q '^CONFIG_MT76x2U=y' .config || { echo "the kernel's settings don't allow the MT7612U driver" >&2; exit 3; }
    make -j"$(nproc)" "$(basename "$image")" 2>&1 | tail -n 3
    cp "$image" "$folder/bzImage"
    cd / && rm -rf "$src"
  fi

  say "The adapter's firmware"
  mkdir -p /lib/firmware/mediatek
  for f in mt7662u.bin mt7662u_rom_patch.bin; do
    [ -s "/lib/firmware/mediatek/$f" ] || curl -fsSL -o "/lib/firmware/mediatek/$f" \
      "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/mediatek/$f"
  done

  say "omdrop-owl (AirDrop's Wi-Fi link, in software)"
  if ! dpkg -s omdrop-owl >/dev/null 2>&1; then
    src=/usr/src/omdrop-owl
    rm -rf "$src"
    git clone -q --depth 1 https://github.com/t4t5/omdrop-owl "$src"
    chown -R "$user" "$src"
    (cd "$src" && sudo -u "$user" tools/build-deb.sh)
    apt-get install -y -q "$src"/omdrop-owl_*.deb
  fi
  # The MT7612U only works in omdrop-owl's "exclusive" mode, and it's a
  # second adapter (Windows keeps its own Wi-Fi), so nothing is lost.
  mkdir -p /etc/omdrop && touch /etc/omdrop/allow-untested
  # The radio helper runs through polkit; WSL has no desktop session to ask
  # in, so this user may run it (only it).
  mkdir -p /etc/polkit-1/rules.d
  cat > /etc/polkit-1/rules.d/49-openhop-omdrop.rules <<EOF
polkit.addRule(function(action, subject) {
  if (action.id.indexOf("omdrop") >= 0 && subject.user == "$user") return polkit.Result.YES;
});
EOF

  say "omdrop (sending and receiving)"
  home=$(getent passwd "$user" | cut -d: -f6)
  plugin="$home/.config/omarchy/plugins/netmojo.omdrop"
  if [ ! -d "$plugin" ]; then
    sudo -u "$user" mkdir -p "$(dirname "$plugin")"
    sudo -u "$user" git clone -q --depth 1 https://github.com/netmojo-systems/omdrop-plugin "$plugin" \
      || sudo -u "$user" git clone -q --depth 1 https://github.com/brentkearney/omdrop-plugin "$plugin"
  fi
  # No password prompts from OpenHop's background commands.
  echo "$user ALL=(root) NOPASSWD: /usr/lib/omdrop/*" > /etc/sudoers.d/openhop-omdrop
  chmod 440 /etc/sudoers.d/openhop-omdrop
  ;;
user)
  downloads=$4
  plugin="$HOME/.config/omarchy/plugins/netmojo.omdrop"
  say "omdrop's set-up"
  "$plugin/bin/omdrop" setup || true
  omdrop="$HOME/.local/bin/omdrop"
  [ -x "$omdrop" ] || { mkdir -p "$HOME/.local/bin"; ln -sf "$plugin/bin/omdrop" "$omdrop"; }
  "$omdrop" dir "$downloads" || true
  "$omdrop" name "$(cat /proc/sys/kernel/hostname)" >/dev/null 2>&1 || true
  ;;
check)
  say "Checking"
  for i in $(seq 1 10); do
    lsusb 2>/dev/null | grep -qi '0e8d:7612' && break
    sleep 1
  done
  lsusb | grep -i '0e8d:7612' || { echo "Linux doesn't see the adapter." >&2; exit 4; }
  for i in $(seq 1 10); do
    iw dev 2>/dev/null | grep -q Interface && break
    sleep 1
  done
  iw dev | grep Interface || { echo "The adapter's driver didn't start (firmware?)." >&2; exit 5; }
  "$HOME/.local/bin/omdrop" doctor || true
  ;;
esac
