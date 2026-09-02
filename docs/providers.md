# Providers

Ainz keeps named provider profiles in `config.toml` under the platform config directory
(`~/Library/Application Support/ainz` on macOS, `~/.config/ainz` on Linux; `AINZ_CONFIG`
overrides the path). The active provider and model remain global CLI overrides through
`--provider`, `--model`, `AINZ_PROVIDER`, and `AINZ_MODEL`.

With no configured model, `ainz` opens an interactive setup flow before starting the first
session. Enter `/config` later to add a provider or switch the active provider and model without
leaving the app.

## Manage profiles

```sh
ainz providers list
ainz providers add NAME --preset ollama
ainz providers add NAME --preset codex --known-model MODEL
ainz providers add NAME --preset claude-code --known-model MODEL
ainz providers use NAME MODEL
ainz providers remove NAME

ainz models list NAME
ainz models list NAME --refresh
ainz models add NAME MODEL
ainz models remove NAME MODEL
```

`models list --refresh` uses the HTTP provider's `/models` endpoint and replaces its stored
model list. Process providers have no common discovery protocol, so their models are managed
explicitly.

## Custom HTTP providers

Any chat-completions-compatible endpoint can be added directly. Credentials remain in an
environment variable; Ainz only stores its name.

```sh
ainz providers add gateway \
  --endpoint https://gateway.example/v1 \
  --api-key-env GATEWAY_API_KEY \
  --known-model example-model
```

HTTP providers support streaming, Ainz tools, usage tracking, and image inputs. Requests have
a 15 second connect timeout and no read timeout, because a local model can take minutes before
its first token; cancel a stuck run with `Ctrl+C`. When no tools are offered, the request omits
`tools` and `tool_choice`, which some compatible servers reject when empty.

## Custom process providers

Process providers receive the full transcript on stdin. Ainz invokes the executable
directly, never through a shell. Arguments support four placeholders:

- `{model}`: active model
- `{workspace}`: canonical workspace path
- `{sandbox}`: `read-only` or `workspace-write`
- `{permission}`: `plan` or `acceptEdits`

```sh
ainz providers add runner \
  --command my-agent \
  --arg run \
  --arg=- \
  --arg=--model \
  --arg='{model}' \
  --known-model example-model
```

Plain process providers return stdout as the assistant response. Add `--json-result` when the
command returns a JSON object whose `result` field contains the final text.

Process providers are adapters around complete coding agents, not raw model APIs. Their own
tools and session behavior remain authoritative; Ainz does not pass them its tool schemas.
They currently return no token usage and omit image content from the rendered transcript.
