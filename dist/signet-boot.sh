#!/bin/sh
# Signet boot hook, launched by udev (rule 98-signet.rules) very early
# in the boot. udev kills its programs after a few seconds: so we
# detach the daemon immediately into its own session.
[ -e /usr/local/signet/signetd ] || exit 0
[ -e /tmp/signetd.ran ] && exit 0
setsid /usr/local/signet/signetd >/dev/null 2>&1 </dev/null &
exit 0
