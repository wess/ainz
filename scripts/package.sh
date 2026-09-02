#!/bin/sh
set -eu

target="${1:?usage: scripts/package.sh TARGET [VERSION]}"
version="${2:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)}"
binary="target/${target}/release/agentx"
archive="agentx-${version}-${target}.tar.gz"

[ -x "$binary" ] || {
  printf 'missing release binary: %s\n' "$binary" >&2
  exit 1
}

mkdir -p dist
staging="$(mktemp -d "${TMPDIR:-/tmp}/agentx-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT HUP INT TERM

install -m 0755 "$binary" "${staging}/agentx"
install -m 0644 LICENSE "${staging}/LICENSE"
install -m 0644 README.md "${staging}/README.md"
tar -C "$staging" -czf "dist/${archive}" agentx LICENSE README.md

if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "$archive" > "${archive}.sha256")
else
  (cd dist && shasum -a 256 "$archive" > "${archive}.sha256")
fi

printf 'dist/%s\n' "$archive"
