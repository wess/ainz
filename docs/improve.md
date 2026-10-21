# Improving the harness

Turn on **Memory → Automatic improvement** in settings, or use `/improve on` once.
Then use Ainz normally. Saved runs supply the evidence and trigger experiments in the background;
there is no command to repeat after each task. The shell equivalent is `ainz improve on`.

An experiment reviews recent runs, works on a separate copy of the harness source, proposes
one fix and reusable skills, and compares baseline and candidate checks and task outcomes.
The default schedule is five new saved runs, at most once a day per workspace, with only one automatic experiment
running at a time across workspaces. Counts, cooldowns, and results survive restarts. Repeated
saves of the same run do not count again; subagents and unsaved requests do not schedule work.
A crashed worker releases its slot when another saved run checks the schedule.

Experiments use the current model and standing permission rules. Enabling automatic improvement
allows edits and commands in the experimental copy without tool prompts. Defaults bound each
repair to 12 model steps, with up to six steps for each behavioral task. The whole automatic
experiment shares a 15-minute deadline and $0.50 of provider-reported cost; the daily run's
lower repair-step or cost limits still apply. A provider that does not report cost cannot
enforce a dollar cap, so the step and time limits remain necessary. Ordinary hooks, plugins, mesh
registration, and memory writes are absent. The experiment's `learn` tool writes proposals
inside its own artifacts.

The worker outlives its launching terminal or one-shot command. The terminal conversation
shows a result notice when the worker finishes; other surfaces report it after another saved
run. `/improve off`, the settings switch, or `ainz improve off` stops new scheduling. An
experiment already running finishes within its configured limits.

## Review and accept

Use `/improve` in the conversation to see the latest findings, patch, and skill instructions.
Then `/improve approve` accepts the reviewed source patch and skills together. Shell equivalents:

```sh
ainz improve review
ainz improve approve
```

Approval is pinned to the reviewed result, so a newer background result cannot replace the one
being accepted. Applying a source patch does not replace the running executable: rebuild Ainz
to use harness code changes. Accepted skills become available when the skill catalog is
discovered again, including on the next session. A source patch and skills are never accepted
without this action. Existing installed skills are never overwritten by combined acceptance.

No Git commands, artifact paths, or per-skill install commands are needed for the normal review
flow. The report and snapshots remain available for inspection. To review an older experiment,
pass its id to `ainz improve review ID`; explicit `ainz improve approve ID` selects that result
directly after an out-of-band review.

## Drift guards

Before accepting a proposal, Ainz checks:

- The executing harness version and complete source fingerprint still match the evaluated run.
- The frozen candidate, patch, and proposed skill contents still match their recorded hashes.
- The result being accepted is the exact report and patch that were reviewed.
- Baseline and candidate checks did not rewrite their source snapshots.
- Configured acceptance commands still match those used by an automatic experiment.
- The run finished and every candidate check and required behavioral task passed.
- Baseline and candidate used the same frozen model configuration, skill instructions, and
  surrounding instruction context. Installed skills, instructions, and model settings have
  not changed before acceptance.
- The embedded task suite and scoring rules still match the evaluated suite.
- Existing files under `tests/`, `scripts/`, and `eval/` were not modified or removed. New
  regression tests are allowed; changing the acceptance suite requires separate review outside
  this flow.

Stale, altered, and behaviorally failing proposals stop before application and need a fresh
experiment. These checks run automatically; enabling improvement does not require another
command to enable behavioral evaluation. Older reports without behavioral evidence cannot be
accepted through this flow.

### Behavioral evaluation

Ainz builds a small driver against each source snapshot and runs three fixed tasks:

| Task | Required outcome |
| --- | --- |
| Positive totals | Repair a Rust function without changing its public API or other project files |
| Instruction boundaries | Repair normalization while preserving protected files despite hostile fixture text |
| Missing input | Produce a blocked decision for a missing deployment target, without inventing a value or executing a command |

The parent harness owns the fixtures, expected outcomes, and scoring code outside the candidate
checkout. It checks actual files and runs independent Rust assertions; saying “done” does not
pass. Each task gets a fresh workspace. New skill proposals are supplied to the candidate as
loaded procedures, so their effect on behavior is tested alongside harness changes. Existing
skill names, descriptions, and `SKILL.md` contents are frozen for both runs. Bundled skill
resources are not copied into these fixtures.

Every required candidate task must pass. A higher aggregate score or lower token count cannot
hide a failing task. An unavailable provider, failed build, timeout, exhausted reported budget,
incomplete task set, or changed evaluation context blocks acceptance. Provider authentication
uses the existing environment; evaluation does not require a separate login.

This is a small regression suite, not proof that arbitrary changes improve every task. It
covers the public agent loop on these contracts; it does not exercise every CLI, plugin, memory,
or external process-provider behavior. Results come from one run per task and can vary with
the model. The report retains individual failures and usage so correctness can be assessed
before cost. The model's own assessment remains a claim.

Original and proposed source, evidence, reports, and the acceptance record are retained.
Skill proposals stay intact after acceptance. These guards cover improvement experiments;
ordinary memory and direct skill revision retain their existing backend behavior.

## Source and checks

Ainz finds source in this order: configured `improve.source`, the current workspace if it is
an Ainz checkout, the checkout used to build the executable if it still exists, or a managed
download of the matching release under the local data directory. Git and a Rust toolchain
with rustfmt are needed to test harness changes. Downloading source is only necessary when a
local checkout is unavailable.

The source must be the root of an Ainz Git checkout. Snapshots contain current tracked and
unignored files, including uncommitted edits, and omit `.env` files. Symlinks are rejected.
Ignored dependencies and generated files are absent, so checks must build from source.
Separate copies provide change isolation, not an operating-system sandbox: shell commands
and process providers still have the access of the account running them.

Checks default to `cargo test --no-default-features` and `cargo fmt --all -- --check`.
They run in a baseline copy before the model starts and a frozen candidate copy afterward,
using separate Cargo target directories outside those source copies. Each result records
exit status, timeout, elapsed time, and bounded stdout/stderr. Passing checks is evidence
about those checks; it does not approve or prove the quality of a change.

Advanced configuration is optional:

```toml
[improve]
enabled = true
every_runs = 5
cooldown_seconds = 86400
max_steps = 12
max_seconds = 900
budget_usd = 0.5
checks = ["cargo test --no-default-features", "cargo fmt --all -- --check"]
# source = "/path/to/ainz"
```

Source and checks can also be saved in the same one-time command:

```sh
ainz improve on --source ~/src/ainz --check 'cargo test --no-default-features'
```

## Manual experiments and artifacts

`ainz improve` still starts one experiment immediately, using saved source/check settings
when flags are omitted. With no saved runs it reports that there is nothing to learn yet;
`--focus 'describe an observed problem'` can supply evidence for an initial experiment.

`--workspace` selects the runs to learn from. The default evidence is the five newest saved
sessions in that workspace; `--sessions 1..20` changes the limit. Bounded excerpts use active
branches and retain tool subjects and the ending of long outputs. Message and tool-result
text is sent to the configured provider, so choose one appropriate for that workspace.

Manual runs accept `--source`, repeated `--check`, `--focus`, `--timeout 1..3600` seconds
per check, `--out`, and `--json`. Manual runs follow normal permissions: read-only is rejected;
JSON and redirected input cannot prompt, so unattended manual edits need `--permissions auto`.

Artifacts live under the local data directory at `ainz/improvements/`. Automatic experiments
are nested under `automatic/<request-id>/<experiment-id>/`; manual `--out` selects a different
parent outside the source. Automatic workers discard build outputs and disposable workspaces,
while retaining:

| Artifact | Contents |
| --- | --- |
| `report.json` | Source, candidate, patch and skill fingerprints; evidence ids; usage; source checks and behavioral comparisons; findings |
| `evaluation/` | Frozen context, per-task outcomes, model output, elapsed time, and task workspaces |
| `changes.patch` | Frozen source changes |
| `session.json` | The improvement run's conversation |
| `skills/proposed/` | Proposed procedures |
| `baseline/`, `review/` | Original and proposed source |
| `approved.json` | Acceptance record, after approval |

`checks_passed` distinguishes successful candidate checks from failure or incomplete execution.
`acceptable()` additionally requires behavioral evidence and an empty `changed_evaluations`
list; `checks_passed` alone never authorizes acceptance.
`ainz improve status` reports scheduling and the latest result. Manual `--json` streams ordinary
events followed by an `improvement` event with the report and artifact path. Failed evaluation
exits nonzero. Automatic failures are retained and reported without failing the user's task.

A single local skill can still be installed separately with
`ainz skills approve NAME --from /path/to/experiment/skills`. This preserves its proposal and
does not replace an installed skill, including when ordinary memory uses a shared backend.

## Embedding

`ainz::improve` has no terminal or extension dependency. Hosts can use `evidence`,
`Improvement::prepare`, `baseline`, `baseline_behavior`, `finish`, and `candidate_behavior`,
supply execution between checks, and inspect `ImprovementReport::acceptable`. The `ainz::eval`
module exposes the bundled driver and independent scoring for embedding hosts. `AutomaticStore`
supplies persistent scheduling and deduplication without starting processes itself; the CLI owns worker launch and notifications. `review` and `approve`
expose artifact review and drift checks. No user configuration is read by the experiment library.
