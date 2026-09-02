# Installing AgentX

## Homebrew

```sh
brew install wess/packages/agentx
```

## Verified installer

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/wess/agentx/main/install.sh | sh
```

The installer supports Intel and Apple Silicon macOS and x86_64 and arm64 Linux. It downloads the
latest release archive, verifies the published SHA-256 file, and installs `agentx` into
`~/.local/bin`. Set `AGENTX_INSTALL_DIR` to choose another destination or `AGENTX_VERSION` to pin a
release. The `search` tool needs [ripgrep](https://github.com/BurntSushi/ripgrep) on `PATH`.

## Cargo

```sh
cargo install --git https://github.com/wess/agentx --tag v0.1.2 --locked
```

## Uninstall

Homebrew installations can be removed with `brew uninstall agentx`. Installer-based installations
can be removed by deleting the single `agentx` binary from the chosen install directory. User
configuration and session data are left intact.
