#!/bin/sh
# Manages the firmware's init script on the Kobo (dev mode, SSH).
#   kobo-rcs.sh status   : which rcS is in place
#   kobo-rcs.sh install  : patches the device's original rcS with dist/rcS.patch
#                          (backs up rcS.orig, sh -n, atomic mv)
#   kobo-rcs.sh restore  : puts rcS.orig back
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
K="$ROOT/tools/kobo-ssh.sh"
case "${1:-status}" in
  status)  "$K" 'md5sum /etc/init.d/rcS /etc/init.d/rcS.orig 2>/dev/null; ls /usr/local/signet/rcs-patched 2>/dev/null && echo "rcS patched" || echo "original rcS"' </dev/null ;;
  install)
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    # Always patch the original: rcS.orig once a backup exists, rcS before that.
    "$K" 'cat /etc/init.d/rcS.orig 2>/dev/null || cat /etc/init.d/rcS' </dev/null > "$TMP/rcS"
    patch --quiet --batch "$TMP/rcS" "$ROOT/dist/rcS.patch" || { echo "dist/rcS.patch does not apply to this firmware's rcS: nothing changed" >&2; exit 1; }
    "$K" 'cat > /tmp/rcS.new && sh -n /tmp/rcS.new && { [ -e /etc/init.d/rcS.orig ] || cp -p /etc/init.d/rcS /etc/init.d/rcS.orig; } && cp /tmp/rcS.new /etc/init.d/rcS.tmp && chmod 755 /etc/init.d/rcS.tmp && mv /etc/init.d/rcS.tmp /etc/init.d/rcS && touch /usr/local/signet/rcs-patched && sync && echo "rcS installed"' < "$TMP/rcS"
    ;;
  restore) "$K" 'cp /etc/init.d/rcS.orig /etc/init.d/rcS && rm -f /usr/local/signet/rcs-patched && sync && echo "original rcS restored"' </dev/null ;;
  *) echo "usage: $0 status|install|restore" >&2; exit 2 ;;
esac
