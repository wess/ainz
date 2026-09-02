# Providers

AgentX keeps named provider profiles in `~/.config/agentx/config.toml`. The active provider
and model remain global CLI overrides through `--provider`, `--model`, `AGENTX_PROVIDER`, and
`AGENTX_MODEL`.

With no configured model, `agentx` opens an interactive setup flow before starting the first
session. Enter `/config` later to add a provider or switch the active provider and model without
leaving the app.

## Manage profiles

```sh
agentx providers list
agentx providers add NAME --preset ollama
agentx providers add NAME --preset codex --known-model MODEL
agentx providers add NAME --preset claude-code --known-model MODEL
agentx providers use NAME MODEL
agentx providers remove NAME

agentx models list NAME
agentx models list NAME --refresh
agentx models add NAME MODEL
agentx models remove NAME MODEL
```

`models list --refresh` uses the HTTP provider's `/models` endpoint and replaces its stored
model list. Process providers have no common discovery protocol, so their models are managed
explicitly.

## Custom HTTP providers

Any chat-completions-compatible endpoint can be added directly. Credentials remain in an
environment variable; AgentX only stores its name.

```sh
agentx providers add gateway \
  --endpoint https://gateway.example/v1 \
  --api-key-env GATEWAY_API_KEY \
  --known-model example-model
```

HTTP providers support streaming, AgentX tools, usage tracking, and image inputs.

## Custom process providers

Process providers receive the full transcript on stdin. AgentX invokes the executable
directly, never through a shell. Arguments support four placeholders:

- `{model}` — active model
- `{workspace}` — canonical workspace path
- `{sandbox}` — `read-only` or `workspace-write`
- `{permission}` — `plan` or `acceptEdits`

```sh
agentx providers add runner \
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
tools and session behavior remain authoritative; AgentX does not pass them its tool schemas.
They currently return no token usage and omit image content from the rendered transcript.
