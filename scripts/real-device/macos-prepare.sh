#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$HOME/LANSyncE2E}"
LARGE_MB="${LARGE_MB:-16}"

SOURCE="$ROOT/source"
TARGET="$ROOT/target"
MANIFEST="$ROOT/manifest.txt"

mkdir -p "$SOURCE/small" "$SOURCE/nested/a/b" "$SOURCE/many" "$TARGET"

printf "hello from macos\n" > "$SOURCE/small/hello.txt"
printf "nested macos report\n" > "$SOURCE/nested/a/b/report.txt"

for n in $(seq 1 20); do
  i="$(printf "%03d" "$n")"
  printf "macos many file %s\n" "$i" > "$SOURCE/many/file-$i.txt"
done

dd if=/dev/urandom of="$SOURCE/large.bin" bs=1048576 count="$LARGE_MB" >/dev/null 2>&1

(
  cd "$SOURCE"
  find . -type f -not -path '*/.lanbridge-history/*' |
    LC_ALL=C sort |
    while IFS= read -r file; do
      rel="${file#./}"
      hash="$(shasum -a 256 "$file" | awk '{print $1}')"
      printf "%s  %s\n" "$hash" "$rel"
    done
) > "$MANIFEST"

echo "LanBridge macOS test data ready"
echo "Source: $SOURCE"
echo "Target: $TARGET"
echo "Manifest: $MANIFEST"
echo "App data: $HOME/Library/Application Support/LanBridge"
echo "TCP service port: 9527"
echo "Discovery UDP: 239.10.10.10:53530"
