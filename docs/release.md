# Release process

AgentX versions come from `Cargo.toml`. A matching `vMAJOR.MINOR.PATCH` tag starts the release
workflow, which verifies the version, builds four native archives, generates a SHA-256 file for each,
and publishes the GitHub release after every platform succeeds.

Supported release targets:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-gnu`

After the assets are public, update the Homebrew formula with their exact checksums and validate an
installation from the tap. Never publish a formula whose URL does not yet exist.
