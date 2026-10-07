#!/bin/sh
# Load the uinput module now and at every boot, then apply the udev rule.
modprobe uinput 2>/dev/null || true
echo uinput > /etc/modules-load.d/openhop-uinput.conf
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --name-match=uinput 2>/dev/null || true
exit 0
