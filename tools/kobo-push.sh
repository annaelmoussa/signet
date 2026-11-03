#!/bin/sh
# Deploys dist/KoboRoot.tgz to the Kobo over SSH, without a reboot.
#
#   tools/kobo-push.sh            installs; active at the next boot
#   tools/kobo-push.sh --relance  installs and restarts the application right away:
#                                 Signet writes its position and exits (code 4), signetd
#                                 restarts the new version. No reader reboot
#                                 (a reboot is still needed for signetd).
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cat "$ROOT/dist/KoboRoot.tgz" | "$ROOT/tools/kobo-ssh.sh" 'cat > /tmp/KoboRoot.tgz && tar xzf /tmp/KoboRoot.tgz -C / && sync && echo "déployé : $(md5sum /usr/local/signet/signet | cut -c1-12)"'
if [ "$1" = "--relance" ]; then
	"$ROOT/tools/kobo-ssh.sh" 'touch /tmp/signet-restart && echo "relance demandée (au plus 2 s)"' </dev/null
fi
