# Installing Ainz

## Homebrew

```sh
brew install wess/packages/ainz
```

## Verified installer

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/wess/ainz/main/install.sh | sh
```

The installer supports Intel and Apple Silicon macOS and x86_64 and arm64 Linux. It downloads the
latest release archive, verifies the published SHA-256 file, and installs `ainz` into
`~/.local/bin`. Set `AINZ_INSTALL_DIR` to choose another destination or `AINZ_VERSION` to pin a
release. The `search` tool needs [ripgrep](https://github.com/BurntSushi/ripgrep) on `PATH`.

## Cargo

```sh
cargo install --git https://github.com/wess/ainz --tag v0.6.2 --locked
```

## Upgrading from AgentX

Ainz reads the old `agentx` configuration, sessions, headers, prompts, skills, plugins, and plugin
approvals, so an existing install keeps its state. New state is written under `ainz` and `.ainz`.
Rename any exported `AGENTX_*` environment variables to `AINZ_*`, and switch the Homebrew formula
with `brew install wess/packages/ainz`.

## Uninstall

Homebrew installations can be removed with `brew uninstall ainz`. Installer-based installations
can be removed by deleting the single `ainz` binary from the chosen install directory. User
configuration and session data are left intact.
