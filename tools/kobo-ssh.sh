#!/bin/sh
# SSH access to the Kobo in dev mode. Tries the Wi-Fi address (KOBO_HOST,
# otherwise the content of dist/kobo-host) then the fallback USB link (169.254.42.2).
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OPTS="-i $HOME/.ssh/signet_kobo -o BatchMode=yes -o ConnectTimeout=4 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
for H in ${KOBO_HOST:-$(cat "$ROOT/dist/kobo-host" 2>/dev/null)} 169.254.42.2; do
  if ssh $OPTS root@$H true </dev/null 2>/dev/null; then exec ssh $OPTS root@$H "$@"; fi
done
echo "kobo injoignable" >&2; exit 1
