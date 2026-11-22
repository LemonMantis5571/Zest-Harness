#!/usr/bin/env bash
set -euo pipefail
repo=$1
binary=$2
version=$3
output=$4
scratch=$5
root="$scratch/debian/zest-desktop"
install -Dm755 "$binary" "$root/usr/bin/zest-desktop"
install -Dm644 "$repo/packaging/gpui/linux/Zest.desktop" "$root/usr/share/applications/dev.zest.app.desktop"
install -Dm644 "$repo/crates/desktop/icons/128x128.png" "$root/usr/share/icons/hicolor/128x128/apps/zest-desktop.png"
install -Dm644 "$repo/LICENSE" "$root/usr/share/doc/zest-desktop/copyright"

# Resolve runtime dependencies from the actual linked binary on the build distribution.
cd "$scratch"
mkdir -p "$root/DEBIAN"
cat > debian/control <<EOF
Source: zest
Section: devel
Priority: optional
Maintainer: zest

Package: zest
Architecture: amd64
Description: Local-first coding workbench
EOF
dependencies=$(dpkg-shlibdeps -O "$root/usr/bin/zest-desktop")
cat > "$root/DEBIAN/control" <<EOF
Package: zest
Version: $version
Architecture: amd64
Section: devel
Priority: optional
Maintainer: zest
Depends: libvulkan1, ${dependencies#shlibs:Depends=}
Description: Zest local-first coding workbench
EOF
dpkg-deb --root-owner-group --build "$root" "$output/Zest_${version}_amd64.deb"

# rpmbuild resolves ELF runtime dependencies from the same executable.
mkdir -p "$scratch/rpmbuild"
cat > "$scratch/zest.spec" <<'SPEC'
Name: zest
Version: %{package_version}
Release: 1
Summary: Local-first coding workbench
License: MIT
BuildArch: x86_64
Requires: libvulkan.so.1()(64bit)
%description
Zest native desktop workbench.
%install
mkdir -p "%{buildroot}"
cp -a "%{payload}/usr" "%{buildroot}/"
%files
/usr/bin/zest-desktop
/usr/share/applications/dev.zest.app.desktop
/usr/share/icons/hicolor/128x128/apps/zest-desktop.png
/usr/share/doc/zest-desktop/copyright
SPEC
rpmbuild -bb --define "_topdir $scratch/rpmbuild" --define "package_version ${version//-/~}" \
  --define "payload $root" --define '_build_id_links none' "$scratch/zest.spec"
cp "$scratch"/rpmbuild/RPMS/x86_64/*.rpm "$output/Zest-$version-1.x86_64.rpm"

# Type 2 AppImages consist of the ELF runtime followed by an AppDir squashfs.
# Pin the runtime by asset ID and digest so a changed upstream build fails closed.
runtime="$scratch/runtime-x86_64"
if [[ -n "${ZEST_APPIMAGE_RUNTIME:-}" ]]; then
  cp "$ZEST_APPIMAGE_RUNTIME" "$runtime"
else
  curl --fail --location --retry 2 --header 'Accept: application/octet-stream' \
    https://api.github.com/repos/AppImage/type2-runtime/releases/assets/657374641 -o "$runtime"
fi
echo "617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46  $runtime" | sha256sum --check
appdir="$scratch/AppDir"
mkdir -p "$appdir"
cp -a "$root/usr" "$appdir/"
ln -s usr/bin/zest-desktop "$appdir/AppRun"
ln -s usr/share/applications/dev.zest.app.desktop "$appdir/dev.zest.app.desktop"
ln -s usr/share/icons/hicolor/128x128/apps/zest-desktop.png "$appdir/zest-desktop.png"
ln -s zest-desktop.png "$appdir/.DirIcon"
mksquashfs "$appdir" "$scratch/app.squashfs" -all-root -noappend -quiet
image="$output/Zest_${version}_amd64.AppImage"
cat "$runtime" "$scratch/app.squashfs" > "$image"
chmod +x "$image"
