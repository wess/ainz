# Release process

AgentX versions come from `Cargo.toml`. Bump it, run `scripts/version.sh` to stamp the new number
into the site and install docs, and commit both together. A matching `vMAJOR.MINOR.PATCH` tag then
starts the release workflow, which verifies the tag against the package version, builds four native
archives, generates a SHA-256 sidecar for each, and publishes a GitHub release with `--verify-tag`
and generated notes once every platform succeeds. A failed build leaves the tag with no release;
delete the tag, fix, and tag again.

AgentX is not published to crates.io: the name there belongs to an unrelated project, so the package
sets `publish = false` and ships as a binary. Source installs use
`cargo install --git https://github.com/wess/agentx --tag vMAJOR.MINOR.PATCH --locked`.

Supported release targets:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-gnu`

Assets are named `agentx-VERSION-TARGET.tar.gz` plus `.sha256`; `install.sh` and the Homebrew
formula both depend on that naming. The formula lives at `wess/homebrew-packages/Formula/agentx.rb`
and declares `ripgrep`, which the `search` tool shells out to. After the assets are public, update
the formula with their exact checksums and validate an installation from the tap. Never publish a
formula whose URL does not yet exist.
