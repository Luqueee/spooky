#!/bin/sh
# Builds dist/spooky_<version>_<arch>.deb from the release binaries in target/release.
# scripts/package.sh builds them first and calls this on Debian-based systems.
#
# SPOOKY_DEB_MAINTAINER overrides the Maintainer field (default: a reserved .invalid
# address, so no real person is named).
set -eu
cd "$(dirname "$0")/.."

for tool in dpkg dpkg-deb objdump; do
  command -v "$tool" >/dev/null || { printf '%s is needed to build a .deb.\n' "$tool" >&2; exit 1; }
done

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
# `0.2.0-dev.1` is a pre-release of 0.2.0: Debian sorts it before it only with a `~`
# (a `-` would start the package revision, which sorts after).
debversion="$(printf '%s' "$version" | sed 's/-/~/')"
arch="$(dpkg --print-architecture)"
maintainer="${SPOOKY_DEB_MAINTAINER:-Spooky contributors <spooky@invalid>}"
# The oldest glibc the binaries can run on is the newest symbol version they use.
glibc="$(objdump -T target/release/spooky target/release/spooky-server \
  | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V -u | tail -n 1)"
[ -n "$glibc" ] || { printf 'Could not read the glibc version the binaries need.\n' >&2; exit 1; }

root="dist/deb/spooky_${debversion}_${arch}"
rm -rf dist/deb
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" \
  "$root/usr/share/icons/hicolor/256x256/apps" "$root/usr/share/metainfo" \
  "$root/usr/share/doc/spooky"
install -m 0755 target/release/spooky target/release/spooky-server "$root/usr/bin/"
install -m 0644 packaging/linux/dev.spooky.terminal.desktop "$root/usr/share/applications/"
install -m 0644 packaging/linux/dev.spooky.terminal.metainfo.xml "$root/usr/share/metainfo/"
install -m 0644 assets/logo-256.png \
  "$root/usr/share/icons/hicolor/256x256/apps/dev.spooky.terminal.png"

# Machine-readable copyright (DEP-5). The Rust dependencies are linked in statically and
# keep their own licenses, which Cargo.lock lists.
{
  printf 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n'
  printf 'Upstream-Name: spooky\n'
  printf 'Source: https://github.com/Luqueee/spooky\n\n'
  printf 'Files: *\n'
  sed -n 's/^Copyright (c) /Copyright: /p' LICENSE | head -n 1
  printf 'License: MIT\n\n'
  printf 'License: MIT\n'
  # The license text, indented as the format asks, with blank lines as a single dot.
  sed -n '3,$p' LICENSE | sed 's/^$/./; s/^/ /'
} >"$root/usr/share/doc/spooky/copyright"
chmod 0644 "$root/usr/share/doc/spooky/copyright"

# The window and graphics libraries are loaded at run time, not linked, so the linker
# does not reveal them: they are listed by hand. Either display server library will do
# for the window, but the X11 ones are needed to fall back to X11.
cat >"$root/DEBIAN/control" <<EOF
Package: spooky
Version: ${debversion}
Architecture: ${arch}
Section: x11
Priority: optional
Maintainer: ${maintainer}
Homepage: https://github.com/Luqueee/spooky
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Depends: libc6 (>= ${glibc}), libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libvulkan1, libwayland-client0, libx11-6, libxcb1, fonts-dejavu-core | fonts-dejavu
Recommends: mesa-vulkan-drivers | vulkan-icd, libxcursor1, libxi6, libxrandr2, openssh-client, xdg-desktop-portal
Suggests: xdg-desktop-portal-kde | xdg-desktop-portal-gnome | xdg-desktop-portal-gtk
Description: Persistent GPU terminal workspaces
 A native dropdown terminal with persistent workspaces, tabs and split panes.
 Shells run on a session server, so closing the window never ends them, locally or
 over SSH. The global shortcut shows and hides the window on X11 and on KDE Plasma
 under Wayland; on other Wayland desktops it only brings the window forward.
EOF

package="dist/spooky_${debversion}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$package"
printf 'Built package: %s\n' "$package"
