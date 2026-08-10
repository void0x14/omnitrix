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

`list` prints each catalog ID, family, and short blurb. `set` accepts only a
canonical catalog ID and writes `strategy = "<id>"`; an unknown ID fails before
the file is changed. `show` resolves the selected ID through the existing
legacy-alias bridge before displaying its catalog record. `explain` prints the
catalog's long help for its ID.

## TUI picker

Use `/routing` or choose **Routing Mode** in the command palette. Type to
fuzzy-narrow mode IDs, titles, and families. Press `Tab` to cycle the family
filter, use Up/Down to select a result, and Enter to persist it. The lower pane
shows the selected catalog entry's one-sentence blurb. Esc closes the picker.

`/omni-routing` remains the legacy strategy/role assignment command. `/connect`
continues to open the provider connection wizard and is unrelated to routing
mode selection.
