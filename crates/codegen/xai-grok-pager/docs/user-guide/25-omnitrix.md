# Omnitrix: All-in-One Operations

Omnitrix is the native operations layer inside the Grok terminal application. It is installed before the first TUI frame, so its status, key, routing, notification, backup, and live-agent surfaces do not depend on a separate synchronisation process or a delayed core bootstrap.

The runtime has three observable phases:

- `basliyor`: command surfaces are available while local metadata is discovered in the background.
- `hazir`: discovery completed and the snapshot is current.
- `kisitli`: commands remain available, but metadata discovery failed or is incomplete.

Use `/omni` to inspect the phase, provider count, active-agent count, session-storage size, and health state.

## Command reference

| Command | Purpose |
| --- | --- |
| `/omni` | Show the current native runtime snapshot. |
| `/omni-dashboard` | List live managed agents as `id`, tier, status, and task. |
| `/omni-dashboard interrupt <id>` | Route a stop request to the selected live agent. |
| `/omni-tasks` | Read persisted Omnitrix tasks from the local task database. |
| `/omni-keys` | Show live/dead key counts grouped by provider. It never prints secret values. |
| `/omni-routing` | Show the selected catalog routing mode. |
| `/omni-routing <mode>` | Persist a routing mode using its catalog ID. |
| `/omni-routing <role> <model>` | Request a role/model assignment; unsupported catalog mappings return an explicit error. |
| `/omni-research <surface|deep|ocean> <question>` | Inject a native `grok_research` request into the current agent turn. |
| `/omni-autonomous <objective>` | Start an autonomous mission using native task, subagent, research, implementation, and verification tools. |
| `/omni-notify test` | Queue a test through configured Telegram and/or phone channels. |
| `/omni-backup now` | Start a session backup in the background. Completion or failure is written to logs. |

`/omni-route` and `/omni-auto` are aliases for `/omni-routing` and `/omni-autonomous`.

## Routing

Use catalog IDs, not descriptive legacy spellings:

| ID | Behaviour |
| --- | --- |
| `rr` | Round robin. |
| `wrr` | Weighted round robin. |
| `fallback-strict` | Ordered fallback chain that stops at the first success. |
| `jep-classic` | Judge, executor, then planner role priority. |

Example:

```text
/omni-routing
/omni-routing wrr
```

The command writes `config/routing.toml` atomically and rejects an unknown mode without changing the existing file. The wider routing catalog is available through the standard `/routing` surface; `/omni-routing` intentionally exposes only the modes supported by the native selector.

## Research and autonomous missions

`/omni-research` is asynchronous at the slash-command boundary. It asks the active agent to call the native research tool once and return its source-backed Markdown report. The modes trade depth for latency:

- `surface`: one narrow pass.
- `deep`: iterative refinement and link following.
- `ocean`: broad, multi-round investigation.

`/omni-autonomous` does not create a second orchestration architecture. It injects one mission into the existing agent runtime and instructs it to decompose work, parallelise independent tasks where safe, research when evidence is required, implement, verify, and continue until the objective is satisfied or an external blocker exists.

## Live agents and interruption

The Omnitrix dashboard uses the same live agent state as the main TUI. IDs shown by `/omni-dashboard` are the accepted interrupt targets. An interrupt temporarily routes through the owning agent view, invokes the native cancellation action, then restores the previous view; it does not fabricate a separate scheduler identity.

## Notifications and backup

Notification channels are resolved from Omnitrix environment configuration. `/omni-notify test` reports missing configuration locally and never blocks the TUI while sending.

`/omni-backup now` schedules the existing session backup engine. “Started” means queued; the command does not claim that the archive completed synchronously. Inspect logs for the final archive path, session count, byte count, and SHA-256 digest.

## Failure behaviour

Omnitrix commands return explicit local messages for missing data, invalid IDs, unsupported mappings, and unconfigured channels. They do not open purchase pages or convert capability errors into paid-plan prompts.

