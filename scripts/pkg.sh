#!/bin/bash
set -euo pipefail

[ "$(uname -s)" = Darwin ] || { echo 'macOS is required' >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
identity="${AINZ_SIGN_IDENTITY:-}"
installer="${AINZ_INSTALLER_IDENTITY:-}"
profile="${AINZ_NOTARY_PROFILE:-}"
if [ -n "$installer" ] && [ -z "$identity" ]; then
  echo 'Set AINZ_SIGN_IDENTITY to sign the embedded binary as well.' >&2
  exit 1
fi
if [ -n "$profile" ] && [ -z "$installer" ]; then
  echo 'Notarization requires AINZ_INSTALLER_IDENTITY.' >&2
  exit 1
fi
cargo build --release --locked
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
payload="$work/root"
mkdir -p "$payload/usr/local/bin" "$payload/usr/local/share/doc/ainz"
install -m 755 target/release/ainz "$payload/usr/local/bin/ainz"
install -m 644 LICENSE README.md "$payload/usr/local/share/doc/ainz/"
cp -R docs "$payload/usr/local/share/doc/ainz/"
xattr -cr "$payload"
arch="$(lipo -archs "$payload/usr/local/bin/ainz")"
case "$arch" in
  arm64|x86_64) ;;
  *) echo "Unsupported installer architecture: $arch" >&2; exit 1 ;;
esac
if [ -n "$identity" ]; then
  # Wasmtime needs executable JIT memory under the hardened runtime.
  cat > "$work/entitlements.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.security.cs.allow-jit</key><true/></dict></plist>
PLIST
  codesign --force --sign "$identity" --identifier io.wess.ainz.cli \
    --options runtime --timestamp --entitlements "$work/entitlements.plist" \
    "$payload/usr/local/bin/ainz"
  codesign --verify --strict "$payload/usr/local/bin/ainz"
fi
pkgbuild --root "$payload" --install-location / --identifier io.wess.ainz.cli \
  --version "$version" --ownership recommended "$work/component.pkg"
mkdir -p "$work/resources"
cat > "$work/resources/conclusion.html" <<'HTML'
<!doctype html><html><body style="font: 13px -apple-system, sans-serif">
<h2>Ainz is installed.</h2>
<p>Open a terminal and run <code>ainz</code> to choose a provider and model.</p>
<p>Use <code>/settings</code> or F2 for preferences and shortcuts.
Start <code>ainz --detach</code> for a background session; Ctrl+B D detaches and
<code>ainz attach</code> reconnects.</p>
<p>The search tool requires ripgrep: <code>brew install ripgrep</code>.</p>
<p>Executable: <code>/usr/local/bin/ainz</code><br>
Manuals: <code>/usr/local/share/doc/ainz</code></p>
</body></html>
HTML
cat > "$work/distribution.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>Ainz $version</title>
  <options customize="never" require-scripts="false" hostArchitectures="$arch"/>
  <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>
  <choices-outline><line choice="default"><line choice="io.wess.ainz.cli"/></line></choices-outline>
  <choice id="default"/>
  <choice id="io.wess.ainz.cli" visible="false"><pkg-ref id="io.wess.ainz.cli"/></choice>
  <pkg-ref id="io.wess.ainz.cli" version="$version">component.pkg</pkg-ref>
  <conclusion file="conclusion.html"/>
</installer-gui-script>
XML
mkdir -p "dist/$version/$arch"
package="$root/dist/$version/$arch/ainz.pkg"
productbuild --distribution "$work/distribution.xml" --package-path "$work" \
  --resources "$work/resources" "$work/unsigned.pkg"
if [ -n "$installer" ]; then
  productsign --sign "$installer" "$work/unsigned.pkg" "$package"
  pkgutil --check-signature "$package"
else
  cp "$work/unsigned.pkg" "$package"
  echo 'Built an unsigned installer.'
fi
if [ -n "$profile" ]; then
  xcrun notarytool submit "$package" --keychain-profile "$profile" --wait
  xcrun stapler staple "$package"
  xcrun stapler validate "$package"
  spctl --assess --type install --verbose=2 "$package"
fi
(cd "$(dirname "$package")" && shasum -a 256 ainz.pkg > ainz.sha256)
printf '%s\n' "$package"
