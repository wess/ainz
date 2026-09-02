#!/bin/sh
# stamps the Cargo.toml version into the places that name it in prose, so a release
# cannot leave the site and install docs pointing at the previous tag
set -eu

cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
[ -n "$version" ] || {
  printf 'could not read the version from Cargo.toml\n' >&2
  exit 1
}

for file in docs/install.md site/index.html site/docs/index.html site/tutorial/index.html; do
  sed -i.bak -E \
    -e "s|--tag v[0-9]+\.[0-9]+\.[0-9]+|--tag v${version}|g" \
    -e "s|AGENTX [0-9]+\.[0-9]+\.[0-9]+|AGENTX ${version}|g" \
    -e "s|AgentX [0-9]+\.[0-9]+\.[0-9]+|AgentX ${version}|g" \
    -e "s|manual · [0-9]+\.[0-9]+\.[0-9]+|manual · ${version}|g" \
    -e "s|<span>[0-9]+\.[0-9]+\.[0-9]+</span>|<span>${version}</span>|g" \
    "$file"
  rm -f "${file}.bak"
done

printf 'stamped %s\n' "$version"
