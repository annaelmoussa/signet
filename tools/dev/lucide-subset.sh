#!/bin/sh
# Regenerates assets/fonts/lucide/lucide-signet.ttf: the Lucide icon font
# (ISC license), reduced to the icons Signet uses (src/icons.rs).
#
#   tools/dev/lucide-subset.sh [version]     (default 1.61.0)
#
# Requires npm and fonttools (pip install fonttools, for pyftsubset).
set -e
VERSION=${1:-1.61.0}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d)
NAMES="a-large-small sun droplet table-of-contents highlighter x chevron-right chevron-left terminal book-open usb book-a trash-2 library moon power bookmark search hash delete sun-moon type arrow-up-down check ellipsis-vertical book-check"
( cd "$WORK" && npm pack "lucide-static@$VERSION" --silent >/dev/null && mkdir pkg && tar xzf "lucide-static-$VERSION.tgz" -C pkg )
FONT="$WORK/pkg/package/font"
UNICODES=$(python3 -I -c "import json,sys; cp=json.load(open(sys.argv[1])); print(','.join('U+%04X' % cp[n] for n in sys.argv[2:]))" "$FONT/codepoints.json" $NAMES)
pyftsubset "$FONT/lucide.ttf" --unicodes="$UNICODES" --output-file="$ROOT/assets/fonts/lucide/lucide-signet.ttf" --layout-features='' --no-hinting --desubroutinize
cp "$WORK/pkg/package/LICENSE" "$ROOT/assets/fonts/lucide/LICENSE"
rm -rf "$WORK"
echo "lucide-signet.ttf : $(wc -c < "$ROOT/assets/fonts/lucide/lucide-signet.ttf") octets, icônes : $NAMES"
