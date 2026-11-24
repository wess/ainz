# Providers

Ainz keeps named provider profiles in `config.toml` under the platform config directory
(`~/Library/Application Support/ainz` on macOS, `~/.config/ainz` on Linux; `AINZ_CONFIG`
overrides the path). Override the startup selection with `--provider`, `--model`,
`AINZ_PROVIDER`, or `AINZ_MODEL`.

With no configured model, `ainz` opens interactive setup. `/model` searches saved models across
providers and pins the selection for the current session, including when you resume it. Press
Esc to keep the current selection. Use `/provider` or **Add or manage providers** in the picker
to open connection setup. Starting Ainz with an explicit provider, endpoint, or model override
uses that connection for the invocation. You can still make a new selection in `/model`.

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

## Automatic model routing

The cross-provider picker and Auto strategies are available starting in Ainz 0.18.0.

1. Add provider profiles with `/provider` or `ainz providers add`. Run `ainz providers list`
   and `ainz models list PROFILE` to find the saved names. Use `ainz models add PROFILE MODEL`
   to save a model that the provider did not discover.
2. Run `ainz doctor` to locate `config.toml`. Add a `[boss]` table with `enabled = true`,
   a router model, and the provider and model pairs the boss may choose. Replace the example
   names with your saved profiles and the model IDs they serve.
3. Start Ainz. New sessions open in Auto with the configured strategy. Choose **Auto · Economy**,
   **Auto · Balanced**, or **Auto · Quality** in `/model` to change the strategy for this
   session. Choose any saved model in the same picker to pin that pair instead.

```toml
[boss]
enabled = true
strategy = "balanced"
router = { provider = "gateway", model = "dispatcher" }
candidates = [
  { provider = "local", model = "quick", cost = "low", quality = "basic" },
  { provider = "gateway", model = "general", cost = "medium", quality = "capable" },
  { provider = "gateway", model = "reasoning", cost = "high", quality = "strong", images = true },
]
```

Each candidate names a saved provider and model. The router uses one HTTP profile and model.
Set `router` explicitly if your active provider is a process adapter. Otherwise the active
pair is the default. Add descriptions under each profile's `model_hints`
to guide specialization. Cost tiers are `low`, `medium`, `high`, and `unknown`. Capability
tiers are `basic`, `capable`, and `strong`. Set `images = true` for candidates that can receive
attached images. Image requirements apply to the task and its active conversation context.

- **Economy** uses the lowest cost tier that meets the task's capability requirement.
- **Balanced** uses the boss's preferred model when it meets the task's requirement.
- **Quality** favors the highest capability tier and uses the boss's preference within that tier.

The boss receives a bounded task brief and the eligible model list. One eligible model skips
the boss call. A failed routing request prefers the configured active model when it's eligible,
then uses the configured tiers. The selected model handles the run, including tools, steering,
and automatic continuations. `/goal` keeps the same model across its follow-up turns. The status
bar shows the chosen pair, and `model_selected` events record the strategy and reason.

The boss and worker share the run's `budget_usd` based on what their providers report. Ainz
checks that total before starting the next model request or tool call. Cost tiers guide
routing choices and can represent local or subscription usage as well as metered API spend.

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
did. `/provider` on a saved provider offers to keep what it uses or change it.

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
when it writes one JSON object per line while it works. Streaming commands can report text
and tool calls while they run and return token counts with the result. The other two modes
stay silent until the command exits.

The Claude Code preset uses the streaming mode. A profile saved by an earlier version on the
buffered `--output-format json` arguments is moved onto it when the config loads.

When a streaming command reports worker status, the local roster shows it. If the stream
closes while a worker is still running, the roster marks its final status unconfirmed.
The provider manages recovery for its own workers.

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

Commands that accept images in a structured input stream get the image itself. Commands that
read prompts as text get `[image file: PATH]`. The path points to the attachment or to a private
copy under `ainz/images` in your cache directory for pasted images and resumed sessions.

The `exec` preset passes `--skip-git-repo-check` so it can run in the chosen workspace outside
a git checkout, and `--json` so worker updates reach the roster. Profiles with the exact older
preset arguments gain these settings on load.

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
in the local session and replay only to the connection and model that produced them. Hosted tools and
WebSocket transport are not implemented. Native Messages retains signed/redacted thinking for
the same connection and model. Switching models carries the ordinary transcript and tool results
to the next model. Reasoning deltas are JSON events; the IRC answer stays separate.

`doctor --check-provider` makes a billable request to the active model with no tools, retries,
or fallback and a 45-second timeout. Listing models alone does not prove inference access.

`budget_usd` in the config, or `--budget-usd AMOUNT`, stops further model requests and tool
execution after the run's reported cost reaches the amount. Providers that omit cost cannot
enforce this limit, and an in-flight request may already have exceeded it. No token prices are
estimated locally.
