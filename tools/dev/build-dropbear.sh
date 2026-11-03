#!/bin/sh
# Builds dist/dropbearmulti: static SSH server for the reader (ARMv7,
# musl, zig cc). The 4.28 fallback firmware has no sshd, so Signet embeds
# its own for dev mode. Usage: tools/dev/build-dropbear.sh [source_dir]
# (by default, downloads dropbear-2024.86 into a temporary directory).
set -e
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SRC=${1:-}
if [ -z "$SRC" ]; then
  WORK=$(mktemp -d)
  curl -sSL -o "$WORK/dropbear.tar.bz2" https://matt.ucc.asn.au/dropbear/releases/dropbear-2024.86.tar.bz2
  echo "e78936dffc395f2e0db099321d6be659190966b99712b55c530dd0a1822e0a5e  $WORK/dropbear.tar.bz2" | shasum -a 256 -c -
  tar xjf "$WORK/dropbear.tar.bz2" -C "$WORK"
  SRC="$WORK/dropbear-2024.86"
fi
cd "$SRC"
export CC="zig cc -target arm-linux-musleabihf -mcpu=cortex_a7"
export AR="zig ar" RANLIB="zig ranlib" STRIP="true"
export CFLAGS="-Os" LDFLAGS="-static -s"
./configure --host=arm-linux-musleabihf --disable-zlib --disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx --disable-loginfunc --disable-pututline --disable-pututxline --enable-static > configure.log 2>&1 || { tail -30 configure.log; exit 1; }
make -j8 PROGRAMS="dropbear dropbearkey dropbearconvert scp" MULTI=1 STATIC=1 > make.log 2>&1 || { tail -40 make.log; exit 1; }
cp dropbearmulti "$ROOT/dist/dropbearmulti"
ls -la "$ROOT/dist/dropbearmulti"
