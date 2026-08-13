# Providers, `/connect`, and `/keys`

The provider workflow has two native TUI surfaces:

- `/connect` selects and activates a provider/model.
- `/keys` manages the encrypted Omnitrix keychain and imports compatible OpenCode credentials automatically.

Neither workflow requires a separate synchronisation command.

## `/connect`

The compact provider picker is centred in the terminal. Keyboard and mouse follow the same transitions:

1. Choose provider mode.
2. Choose a provider.
3. Choose a model when the provider exposes more than one.
4. Supply or select credentials when required.
5. Connect.

Clicking a row performs the same action as selecting it and pressing `Enter`. Clicking footer controls dispatches their labelled keyboard action. `Esc` returns to the preceding step or closes the picker. Provider discovery comes from the models.dev catalog; model IDs and authentication requirements are not hardcoded into the view.

Grok browser login and OAuth remain available. Capability or quota failures are shown as neutral errors; the UI does not contain paid-plan redirects.

## `/keys` automatic OpenCode import

Opening `/keys` starts OpenCode credential discovery in the background when the keychain is unlocked. If it is locked, discovery starts immediately after a successful unlock.

The importer:

1. Resolves the installed OpenCode `auth.json` path.
2. Reads API-key records only; OAuth and well-known non-key records are ignored.
3. Imports every provider record into the encrypted Omnitrix keychain.
4. Preserves an existing Omnitrix provider key unless overwrite was explicitly requested elsewhere.
5. Saves once after the merge.
6. Fetches the current models.dev catalog.
7. Activates the first usable OpenCode provider/model, preferring OpenCode's configured `model`, `small_model`, or `smallModel` entries.

Automatic activation is restricted to provider IDs found in that OpenCode scan. An unrelated pre-existing keychain entry cannot be selected accidentally. If no imported provider has a catalog model, the keys remain stored and the UI reports that activation was not possible.

Secrets are masked in browse mode. A full key exists in memory only in Reveal mode and is cleared when Reveal closes.

## `/keys` controls

| Key | Action |
| --- | --- |
| `↑` / `↓`, `j` / `k` | Move selection. |
| `r` | Reveal the selected key after keychain access is available. |
| `a` | Add a provider key. |
| `e` | Edit model, base URL, or key for the selected record. |
| `x` | Remove the selected key after confirmation. |
| `X` | Remove the selected category after confirmation. |
| `c` | Browse categories and set the active/default category. |
| `E` | Export all keys or one category to an encrypted archive. |
| `I` | Import an encrypted Omnitrix archive. |
| `S` | Open stack import/export for supported external tools. |
| `Tab` / arrows | Move between form fields. |
| `Ctrl+T` | Show or mask the current password/key editor. |
| `Enter` | Execute the focused action. |
| `Esc` | Back or close. |

Mouse selection and footer clicks are translated to these same key actions, so pointer and keyboard paths share validation and side effects.

## Storage and conflict rules

The Omnitrix keychain encrypts secrets at rest. Imports are conflict-safe by default: an existing provider/category mapping wins and the incoming duplicate is skipped. Export archives are password-protected and can be scoped to all keys or selected categories. Key summaries and `/omni-keys` expose counts only, never secret material.

## Troubleshooting

- “OpenCode API records are current” means the compatible entries already existed; catalog activation still runs.
- “No activatable model” means keys were imported but no matching provider/model pair exists in the fetched catalog.
- A missing OpenCode file does not prevent manual keychain use.
- If a provider connects but a request fails, verify the provider's base URL, model ID, and key scope in `/keys`, then reconnect through `/connect`.

