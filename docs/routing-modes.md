# Routing modes

Routing mode definitions, their families, one-line blurbs, and detailed help
are loaded from [`config/routing_modes.toml`](../config/routing_modes.toml).
The selected ID is persisted in [`config/routing.toml`](../config/routing.toml)
through the P1.4 routing configuration bridge.

## CLI

Run these commands from the repository/workspace root containing `config/`:

```text
grok routing list
grok routing set <id>
grok routing show
grok routing explain <id>
```

[`list`](../crates/codegen/xai-grok-pager/src/routing_cmd.rs#L58) prints each
catalog ID, family, and short blurb. [`set`](../crates/codegen/xai-grok-pager/src/routing_cmd.rs#L75)
accepts only a canonical catalog ID and writes `strategy = "<id>"`; an unknown
ID fails before the file is changed. [`show`](../crates/codegen/xai-grok-pager/src/routing_cmd.rs#L92)
resolves the selected ID through the existing legacy-alias bridge before
displaying its catalog record. [`explain`](../crates/codegen/xai-grok-pager/src/routing_cmd.rs#L58)
prints the catalog's long help for its ID.

## TUI picker

Use [`/routing`](../crates/codegen/xai-grok-pager/src/slash/commands/routing.rs#L15)
or choose [**Routing Mode** in the command palette](../crates/codegen/xai-grok-pager/src/views/modal.rs#L492).
Type to [fuzzy-narrow](../crates/codegen/xai-grok-pager/src/views/routing_picker.rs#L37)
mode IDs, titles, and families. Press [`Tab`, Up/Down, Enter, or Esc](../crates/codegen/xai-grok-pager/src/views/routing_picker.rs#L68)
to filter, select, persist, or close. The [lower pane](../crates/codegen/xai-grok-pager/src/views/routing_picker.rs#L168)
shows the selected catalog entry's one-sentence blurb.

[`/omni-routing`](../crates/codegen/xai-grok-pager/src/slash/commands/omni_routing.rs#L66)
remains the legacy strategy/role assignment command.
[`/connect`](../crates/codegen/xai-grok-pager/src/slash/commands/connect.rs#L9)
continues to open the provider connection wizard and is unrelated to routing
mode selection.
