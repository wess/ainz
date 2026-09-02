#!/bin/sh
set -eu

repository="wess/ainz"
install_dir="${AINZ_INSTALL_DIR:-${HOME}/.local/bin}"
base_url="https://github.com/${repository}/releases"

fail() {
  printf 'ainz: %s\n' "$*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

case "$(uname -s)" in
  Darwin) platform="apple-darwin" ;;
  Linux) platform="unknown-linux-gnu" ;;
  *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
  arm64|aarch64) architecture="aarch64" ;;
  x86_64|amd64) architecture="x86_64" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

target="${architecture}-${platform}"
if [ -n "${AINZ_VERSION:-}" ]; then
  version="${AINZ_VERSION#v}"
else
  release_url="$(curl --proto '=https' --tlsv1.2 -LsS -o /dev/null -w '%{url_effective}' "${base_url}/latest")"
  version="${release_url##*/}"
  version="${version#v}"
fi

[ -n "$version" ] || fail "could not determine the latest version"
archive="ainz-${version}-${target}.tar.gz"
download_url="${base_url}/download/v${version}/${archive}"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/ainz-install.XXXXXX")"
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

printf 'Downloading Ainz %s for %s...\n' "$version" "$target"
curl --proto '=https' --tlsv1.2 -fLsS "$download_url" -o "${temporary_dir}/${archive}"
curl --proto '=https' --tlsv1.2 -fLsS "${download_url}.sha256" -o "${temporary_dir}/${archive}.sha256"

(
  cd "$temporary_dir"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${archive}.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${archive}.sha256"
  else
    fail "sha256sum or shasum is required to verify the download"
  fi
  tar -xzf "$archive"
)

mkdir -p "$install_dir"
install -m 0755 "${temporary_dir}/ainz" "${install_dir}/ainz"
printf 'Installed Ainz %s to %s/ainz\n' "$version" "$install_dir"

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) printf 'Add %s to PATH to run ainz.\n' "$install_dir" ;;
esac
