#!/bin/sh
# Builds dist/KoboRoot.tgz: Signet code only.
# The Kobo unpacks this file at the root (/) on the next boot after
# USB eject, then deletes it. Paths must be relative ("./usr/…").
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET="$ROOT/target/armv7-unknown-linux-musleabihf/release"
STAGE=$(mktemp -d)
chmod 755 "$STAGE"
trap 'rm -rf "$STAGE" "$STAGE.files"' EXIT

mkdir -p "$STAGE/usr/local/signet" "$STAGE/etc/udev/rules.d"
for bin in signetd signet-probe signet-fbtest signet; do
  cp "$TARGET/$bin" "$STAGE/usr/local/signet/$bin"
done
cp "$ROOT/dist/signet-boot.sh" "$STAGE/usr/local/signet/signet-boot.sh"
cp "$ROOT/dist/sshd_config" "$STAGE/usr/local/signet/sshd_config"
# Mac public key for dev mode (never committed: dist/authorized_keys is ignored).
[ -e "$ROOT/dist/authorized_keys" ] || echo "warning: no dist/authorized_keys, dev mode SSH will refuse every login" >&2
[ -e "$ROOT/dist/authorized_keys" ] && cp "$ROOT/dist/authorized_keys" "$STAGE/usr/local/signet/authorized_keys"
# Static SSH server (Dropbear, built by tools/dev/build-dropbear.sh;
# dist/dropbearmulti is not committed): the fallback firmware has no sshd.
[ -e "$ROOT/dist/dropbearmulti" ] && cp "$ROOT/dist/dropbearmulti" "$STAGE/usr/local/signet/dropbearmulti"
cp "$ROOT/dist/98-signet.rules" "$STAGE/etc/udev/rules.d/98-signet.rules"
chmod 755 "$STAGE"/usr/local/signet/*
chmod 600 "$STAGE/usr/local/signet/sshd_config" ; [ -e "$STAGE/usr/local/signet/authorized_keys" ] && chmod 600 "$STAGE/usr/local/signet/authorized_keys"
chmod 644 "$STAGE/etc/udev/rules.d/98-signet.rules"

# Clean archive: files only (otherwise busybox tar would reapply the
# archive's directory permissions to / and /usr), no macOS
# metadata (._*), owned by root.
mkdir -p "$ROOT/dist"
( cd "$STAGE" && find . -type f | sort > "$STAGE.files" && \
  COPYFILE_DISABLE=1 tar --format ustar --uid 0 --gid 0 --uname root --gname root -n \
    -czf "$ROOT/dist/KoboRoot.tgz" -T "$STAGE.files" )
echo "→ $ROOT/dist/KoboRoot.tgz"
tar -tzvf "$ROOT/dist/KoboRoot.tgz"
