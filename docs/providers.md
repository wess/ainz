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
ainz providers add NAME --preset lite-llm
ainz providers add NAME --preset codex --known-model MODEL
ainz providers add NAME --preset claude-code --known-model MODEL
ainz providers use NAME MODEL
ainz providers remove NAME

ainz models list NAME
ainz models list NAME --refresh
ainz models add NAME MODEL
ainz models remove NAME MODEL
```

Setup asks rather than expecting a model name to be remembered: an HTTP provider is asked for
`/models` and the answer becomes the list to pick from, and `models list --refresh` replaces a
stored list the same way. Process providers have no common discovery protocol, so the presets
offer what can be known — the aliases the CLI's own help documents, plus the model that tool is
already configured with on this machine (`~/.codex/config.toml`, `~/.claude/settings.json`).
Every list ends in a row for naming a model by hand.

## LiteLLM

A [LiteLLM](https://docs.litellm.ai/) proxy puts every provider it fronts behind one
chat-completions endpoint, so one Ainz profile covers all of them. The preset points at
`http://127.0.0.1:4000/v1` and reads the key from `LITELLM_API_KEY`; setup asks for the endpoint
and the variable name, then lists the models the proxy serves.

```sh
ainz providers add litellm --preset lite-llm --api-key-env LITELLM_API_KEY
ainz models list litellm --refresh
ainz providers use litellm gpt-5.6-sol
```

Model names are whatever the proxy exposes, so its `model_list` is the source of truth. Ainz
stores only the variable name; the key stays in the environment.

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

A connection failure, a 408 or 429, or a 5xx status is retried with exponential backoff (roughly
500ms, 1s, 2s, 4s, capped at 8s, with jitter so concurrent runs don't retry in lockstep); a
`Retry-After` header is honoured when it names a plain number of seconds. Any other 4xx fails the
turn immediately, since retrying it would not change the answer. `provider_retries` in
`config.toml` sets how many of these retries are made beyond the first try (default 3, so 4
attempts total); a retry only ever happens before the response has produced any text, so a run
already streaming an answer is never replayed.

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
command returns a JSON object whose `result` field contains the final text, or `--stream-json`
when it writes one JSON object per line while it works, the way `claude -p --output-format
stream-json` does. A streaming command reports itself as it runs: its text appears a piece at
a time, its tool calls show up in the transcript and the status bar, and the run's token counts
come back with the result. The other two modes stay silent until the command exits.

The Claude Code preset uses the streaming mode. A profile saved by an earlier version on the
buffered `--output-format json` arguments is moved onto it when the config loads.

Process providers are adapters around complete coding agents, not raw model APIs. Their own
tools and session behavior remain authoritative; Ainz does not pass them its tool schemas.
They omit image content from the rendered transcript, and report token usage only in the
streaming mode.
