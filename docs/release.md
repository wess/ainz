# Release process

Ainz versions come from `Cargo.toml`. Bump it, run `scripts/version.sh` to stamp the new number
into the site and install docs, and commit both together. A matching `vMAJOR.MINOR.PATCH` tag then
starts the release workflow, which verifies the tag against the package version, builds four native
archives, generates a SHA-256 sidecar for each, and publishes a GitHub release with `--verify-tag`
and generated notes once every platform succeeds. A failed build leaves the tag with no release;
delete the tag, fix, and tag again.

Ainz is not published to crates.io: the name there belongs to an unrelated project, so the package
sets `publish = false` and ships as a binary. Source installs use
`cargo install --git https://github.com/wess/ainz --tag vMAJOR.MINOR.PATCH --locked`.

Supported release targets:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-gnu`

Assets are named `ainz-VERSION-TARGET.tar.gz` plus `.sha256`; `install.sh` and the Homebrew
formula both depend on that naming. The formula lives at `wess/homebrew-packages/Formula/ainz.rb`
and declares `ripgrep`, which the `search` tool shells out to. After the assets are public, update
the formula with their exact checksums and validate an installation from the tap. Never publish a
formula whose URL does not yet exist.

## Website

The static site lives in `site/`; no build or package installation is required. Both designers
use browser modules, with reusable format and installation code under `site/assets/`. Run
`bun test tests/site` to check exports, installation commands, and internal links. Preview with
`python3 -m http.server 8765 --directory site`, then visit `http://localhost:8765/`.

`scripts/mascot.py` regenerates the terminal artwork and the shared website mascot assets.
Keep both outputs together when changing the mascot. Check Masthead Studio and Theme Designer
at desktop and narrow widths, including editing, reload, copy, download, and invalid input.

`.github/workflows/pages.yml` uploads `site/` to GitHub Pages after relevant changes reach `main`.
A manual dispatch deploys the checked-out branch contents; it cannot publish local uncommitted
files. Confirm the Pages workflow succeeds and check the live designer pages after publishing.
