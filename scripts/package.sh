#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --workspace
case "$(uname -s)" in
  Darwin)
    app="dist/Spooky.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp target/release/spooky target/release/spooky-server "$app/Contents/MacOS/"
    cp packaging/macos/Info.plist "$app/Contents/Info.plist"
    plutil -lint "$app/Contents/Info.plist"
    printf 'Built unsigned application: %s
' "$app"
    ;;
  Linux)
    out="dist/spooky-linux-$(uname -m)"
    mkdir -p "$out/bin" "$out/share/applications"
    cp target/release/spooky target/release/spooky-server "$out/bin/"
    cp packaging/linux/spooky.desktop "$out/share/applications/"
    tar -czf "$out.tar.gz" -C "$out" .
    printf 'Built archive: %s.tar.gz
' "$out"
    ;;
  *) printf 'Packaging is currently supported on macOS and Linux only.
' >&2; exit 1 ;;
esac
