#!/bin/sh
# Load the uinput module now and at every boot, then apply the udev rule.
modprobe uinput 2>/dev/null || true
echo uinput > /etc/modules-load.d/openhop-uinput.conf
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --name-match=uinput 2>/dev/null || true

# Let other computers reach OpenHop if a firewall is on:
# TCP 24850-24890 (connections; a higher port is used if 24850 is busy), UDP 24851 (discovery).
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
  ufw allow 24850:24890/tcp comment OpenHop >/dev/null 2>&1 || true
  ufw allow 24851/udp comment OpenHop >/dev/null 2>&1 || true
fi
if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port=24850-24890/tcp >/dev/null 2>&1 || true
  firewall-cmd --permanent --add-port=24851/udp >/dev/null 2>&1 || true
  firewall-cmd --reload >/dev/null 2>&1 || true
fi

# Wi-Fi power saving makes the adapter doze between packets, which delays
# keystrokes by up to a few hundred milliseconds. Turn it off (NetworkManager).
if [ -d /etc/NetworkManager/conf.d ]; then
  printf '[connection]\n# Added by OpenHop: keeps typing on other computers instant.\nwifi.powersave = 2\n' > /etc/NetworkManager/conf.d/60-openhop-wifi-powersave.conf
  if command -v iw >/dev/null 2>&1; then
    for dev in $(iw dev 2>/dev/null | awk '/Interface/ {print $2}'); do iw dev "$dev" set power_save off 2>/dev/null || true; done
  fi
fi
exit 0
