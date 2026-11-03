#!/bin/sh
# Reboots the Kobo (dev mode) over SSH, using the daemon's protected reboot
# (`signetd reboot`: partition read-only, hardware watchdog armed,
# Wi-Fi unloaded). A daemon too old to know it: plain `reboot`.
# The reading position is saved beforehand (Signet receives SIGTERM).
#
#   tools/kobo-reboot.sh        then wait for /tmp/signet-ip (≈ 40 s)
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
"$ROOT/tools/kobo-ssh.sh" '
if grep -q "chien de garde" /usr/local/signet/signetd 2>/dev/null; then
  echo "redémarrage protégé"
  setsid sh -c "sleep 1; /usr/local/signet/signetd reboot" >/dev/null 2>&1 </dev/null &
else
  echo "redémarrage simple (démon ancien)"
  sync; setsid sh -c "sleep 1; reboot" >/dev/null 2>&1 </dev/null &
fi' </dev/null
