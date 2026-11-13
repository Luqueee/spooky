#!/bin/sh
# Builds a distributable Spooky: dist/Spooky.app (+ zip) on macOS, a tarball on Linux.
#
# macOS signing, chosen with environment variables:
#   (none)                    ad-hoc signature: runs here, Gatekeeper refuses it elsewhere
#   SPOOKY_SIGN_IDENTITY      a "Developer ID Application: ..." identity from the keychain
#   SPOOKY_NOTARY_PROFILE     a `notarytool store-credentials` profile; submits and staples
#                             (needs SPOOKY_SIGN_IDENTITY)
set -eu
cd "$(dirname "$0")/.."
cargo build --locked --release --workspace
case "$(uname -s)" in
  Darwin)
    app="dist/Spooky.app"
    identity="${SPOOKY_SIGN_IDENTITY:--}"
    if [ -n "${SPOOKY_NOTARY_PROFILE:-}" ] && [ "$identity" = "-" ]; then
      printf 'SPOOKY_NOTARY_PROFILE needs SPOOKY_SIGN_IDENTITY: only a Developer ID signature can be notarized.\n' >&2
      exit 1
    fi
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp target/release/spooky target/release/spooky-server "$app/Contents/MacOS/"
    cp packaging/macos/Info.plist "$app/Contents/Info.plist"
    plutil -lint "$app/Contents/Info.plist"

    iconset="$(mktemp -d)/Spooky.iconset"
    mkdir "$iconset"
    for size in 16 32 128 256 512; do
      sips -z "$size" "$size" packaging/macos/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
      sips -z "$((size * 2))" "$((size * 2))" packaging/macos/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/Spooky.icns"
    rm -rf "$(dirname "$iconset")"

    # Sign the inner executables first, then the bundle that contains them.
    if [ "$identity" = "-" ]; then
      options=""
    else
      options="--options runtime --timestamp"
    fi
    for target in "$app/Contents/MacOS/spooky-server" "$app/Contents/MacOS/spooky" "$app"; do
      # shellcheck disable=SC2086
      codesign --force $options --sign "$identity" "$target"
    done
    codesign --verify --strict --verbose=2 "$app"

    archive="dist/Spooky-macos-$(uname -m).zip"
    rm -f "$archive"
    if [ -n "${SPOOKY_NOTARY_PROFILE:-}" ]; then
      ditto -c -k --keepParent "$app" "$archive"
      xcrun notarytool submit "$archive" --keychain-profile "$SPOOKY_NOTARY_PROFILE" --wait
      xcrun stapler staple "$app"
      rm -f "$archive"
      printf 'Notarized and stapled.\n'
    fi
    ditto -c -k --keepParent "$app" "$archive"
    if [ "$identity" = "-" ]; then
      printf 'Built ad-hoc signed application: %s (%s)\n' "$app" "$archive"
    else
      printf 'Built signed application: %s (%s)\n' "$app" "$archive"
    fi
    ;;
  Linux)
    out="dist/spooky-linux-$(uname -m)"
    rm -rf "$out"
    mkdir -p "$out/bin" "$out/share/applications"
    cp target/release/spooky target/release/spooky-server "$out/bin/"
    cp packaging/linux/dev.spooky.terminal.desktop "$out/share/applications/"
    tar -czf "$out.tar.gz" -C "$out" .
    printf 'Built archive: %s.tar.gz\n' "$out"
    ;;
  *) printf 'Packaging is currently supported on macOS and Linux only.\n' >&2; exit 1 ;;
esac
