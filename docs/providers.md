# Providers

Ainz keeps named provider profiles in `config.toml` under the platform config directory
(`~/Library/Application Support/ainz` on macOS, `~/.config/ainz` on Linux; `AINZ_CONFIG`
overrides the path). The active provider and model remain global CLI overrides through
`--provider`, `--model`, `AINZ_PROVIDER`, and `AINZ_MODEL`.

With no configured model, `ainz` opens an interactive setup flow before starting the first
session. Enter `/model` later to add a provider or switch the active provider and model without
leaving the app.

## Manage profiles

```sh
ainz providers list
ainz providers add NAME --preset ollama
ainz providers add NAME --preset ollama-cloud
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

## Ollama Cloud

Ainz can also talk directly to Ollama Cloud's OpenAI-compatible endpoint using an API key stored in
`OLLAMA_API_KEY`.

```sh
ainz providers add ollama-cloud --preset ollama-cloud
```

## Credentials

A provider's key comes from one of three places, chosen during setup rather than typed from
memory:

- an environment variable, by name — Ainz stores the name, never the value, and reads it when
  a request is made;
- a secret Synapse already holds, stored as `apis.Something` and the variable it resolves into.
  Ainz asks Synapse for the value when it needs one, so a secret scoped to another directory is
  simply unavailable here rather than an error;
- a token typed once, kept in the operating system's keychain (macOS `security`, Linux
  `secret-tool`) under the provider's name. The config file records that the token is in the
  keychain, and nothing else.

```toml
[providers.litellm.credential]
from = "synapse"
secret = "apis.OpenRouter"
var = "OPENROUTER_API_KEY"
```

A profile written before this existed keeps its `api_key_env` name and resolves exactly as it
did. `/model` on a saved provider offers to keep what it uses or change it, which is the usual
reason to open setup again.

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
- `{permission}`: `plan`, `default`, or `acceptEdits`

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

### Claude Code asks through Ainz

The preset also passes `--permission-prompt-tool stdio`. Instead of refusing a tool call it
has no one to ask about, `claude -p` sends the request back to Ainz and waits. Ainz decides
it the same way it decides its own calls: standing rules first, then the mode, then the
approval prompt in the buffer. Claude's tool names are translated first (`Bash` is `shell`,
`Edit` is `edit`, `WebFetch` is `fetch`, `mcp__server__tool` is `mcp`), so a rule such as
`shell(git status*)` matches the call it was written for.

Claude decides some calls before it asks anyone. Anything allowed in your own Claude
settings (`permissions.allow` in `~/.claude/settings.json`, or a `defaultMode` of `auto` or
`bypassPermissions`) runs without reaching Ainz. So do the commands Claude treats as safe on
its own, such as a plain `echo`. To make Ainz the only one deciding, add
`--setting-sources project,local` to the profile's arguments. That leaves out your user-level
Claude settings, including the hooks and MCP servers defined there.

In ask mode a bridged command runs in Claude's `default` permission mode and asks. Without the
bridge it would have no way to ask, so it runs in `plan` mode instead. Auto mode passes
`acceptEdits`; read-only passes `plan`. A profile saved on the previous preset gains the flag
when the config loads. A hand-written profile is left alone.

Process providers are adapters around complete coding agents, not raw model APIs. Their own
tools and session behavior remain authoritative; Ainz does not pass them its tool schemas.
They report token usage only in the streaming mode.

Images go to a command in the form it reads. A stream-json command such as Claude Code gets
the image itself. A text command such as Codex gets `[image file: PATH]`, which its own model
opens: the file the image was attached from, or, for a pasted image or one in a resumed session,
a copy written once to `ainz/images` under your cache directory, readable only by you.

The Codex preset passes `--skip-git-repo-check`, because Ainz has already chosen the workspace
and Codex otherwise refuses to run outside a git checkout it trusts. A profile saved on the
previous preset gains the flag when the config loads.

## When a reply stops early

A Chat Completions request asks for replies of up to 32,000 tokens. A request that names no
limit gets the server's, and a LiteLLM proxy in front of Claude picks 4,096. Set
`max_output_tokens` on a profile to change it, or `0` to send no limit:

```toml
[providers.litellm]
max_output_tokens = 64000
```

`ainz providers add NAME --max-output-tokens N` sets it at creation. If a model refuses the
limit, for example one whose ceiling is 16,384, the request is sent again without it once,
and later requests leave it out. The Messages wire always sends its own limit, and Responses
leaves the limit to the model.

A reply that reaches its output limit is kept and marked `[cut off: this reply reached its
length limit]`, and the run continues by itself. The next request tells the model to carry on
where it stopped and to send any call it was cut off in the middle of in smaller pieces. The
continuation keeps the run's permissions and instructions, and counts toward the step limit
and `budget_usd`. After three continuations in a row without a successful tool call, the run
pauses and says so. Steering takes precedence, and cancelling still stops the run.

A reply that breaks off partway, through a dropped connection, an overloaded upstream, or a
stream that ends early, is requested again after 2 and then 8 seconds. A notice explains
why the text is repeated. Process providers are never retried this way, because the
command may already have acted.

Context use is estimated from the bytes sent until the provider reports a token count. After
that, the ratio the provider reported sizes the next request, and the status bar's `ctx` field
shows the share of the window the last request took. A refusal for length lowers the
estimate before the history is compacted and the request is retried.

## Native APIs and fallback

The OpenAI preset uses Responses. Anthropic uses Messages, including signed thinking blocks;
OpenRouter uses Chat Completions. Existing profiles remain on Chat Completions.

```sh
ainz login
ainz providers add openai --preset openai
ainz providers add anthropic --preset anthropic --thinking-tokens 1024
ainz providers add router --preset openrouter
ainz providers add backup --endpoint https://gateway.example.org/v1 \
  --wire-api responses --model BACKUP_MODEL --reasoning-effort high
ainz providers fallback anthropic backup
ainz doctor --check-provider
```

`--wire-api chat|responses|messages` also works on presets, so a compatible gateway can use
Responses. `--api-key-env` overrides a preset's credential variable. `--model` on `providers add`
sets the profile's default model; `--known-model` adds entries to its selection list.

`/settings` includes Provider API, Reasoning effort, and Fallback providers under Connection.
Reasoning effort applies only to Responses: `none`, `minimal`, `low`, `medium`, `high`, or
`xhigh`; leave it unset for the model's default. A Messages thinking budget must be between
1024 and 128000 tokens. The upstream decides which options its model supports.

The active provider is tried first. Each named fallback can have its own `model`; without one
it uses the active model. Fallback is explicit and buffers each attempt's events, so a failed
partial reply never appears alongside a successful backup answer. This delays visible output
until an attempt completes when fallback is enabled. Run `ainz providers fallback` with no names
to clear the chain. Repeated `--fallback-provider NAME` flags override it for one run, as does
`AINZ_FALLBACK_PROVIDERS` with comma-separated names. `AINZ_ENDPOINT` overrides the selected
HTTP profile's endpoint as well as the legacy endpoint.

Responses requests set `store = false`. Output items, including encrypted reasoning, persist
in the local session and replay only to the model that produced them. Hosted tools and
WebSocket transport are not implemented. Native Messages retains signed/redacted thinking for
the next tool turn. Reasoning deltas are JSON events; the IRC answer stays separate.

`doctor --check-provider` makes a billable request to the active model with no tools, retries,
or fallback and a 45-second timeout. Listing models alone does not prove inference access.

`budget_usd` in the config, or `--budget-usd AMOUNT`, stops further model requests and tool
execution after the run's reported cost reaches the amount. Providers that omit cost cannot
enforce this limit, and an in-flight request may already have exceeded it. No token prices are
estimated locally.
