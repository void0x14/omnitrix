# Omnitrix Zig TUI UI/UX Completion Design

**Date:** 2026-08-20  
**Status:** UI-only design; core/runtime integration is explicitly out of scope.

## Intent

Omnitrix will reproduce the visible spatial language and interaction rhythm of the
OpenCode TUI while keeping its implementation native to Zig. The reference is
used for layout, hierarchy, focus feedback, keyboard discoverability, and visual
density only. No OpenCode/Rust state machine, renderer, provider contract,
permission model, worker process, IPC, or memory ownership model is copied.

The UI gate must be complete before any runtime/core integration begins.

## Current evidence and gap

The current Zig TUI already renders a welcome composer, a session with
Conversation/Changes/Diff tabs, a right sidebar, a prompt, a footer, a searchable
palette, select/alert modals, scrollback, mouse input plumbing, alternate-screen
terminal handling, and a correct `OMNITRIX` home brand.

The current implementation is still a deterministic demo surface. It does not
yet provide a complete user-visible state model for login/authentication,
loading/error/blocked states, safe streaming, collapsible tool blocks,
contextual focus/shortcut feedback, narrow-terminal fallbacks, or durable
selection across resize. These are UI requirements, not a reason to start the
core.

## UI completion boundary

The completed UI must visibly cover these states:

1. **Welcome/auth**
   - `OMNITRIX` wordmark keeps the current reference placement and typography.
   - Login/provider choice, pending/browser-or-device handoff, success, cancel,
     and failure states have distinct visible feedback.
   - No credential value is rendered or persisted by the UI fixture.

2. **Home composer**
   - Empty, focused, typed, multiline, shell-mode, history, and validation/error
     states.
   - Contextual shortcut hints change with focus and mode.
   - Prompt text uses an owned bounded buffer and keeps cursor visibility correct.

3. **Session**
   - Conversation header, user/assistant/system blocks, markdown/code, thinking,
     tool-use, tool-result, error, and streaming states.
   - Tool blocks support collapsed summary and expanded detail.
   - A streaming message renders only a safe committed tail and never exposes
     half-formed markup as a broken layout.

4. **Side surfaces**
   - Conversation, Changes, and Diff tabs.
   - Changed-file rows include status, path, additions/deletions, and actor/state
     labels where data is available.
   - Diff supports hunk selection and a fullscreen fallback when the terminal is
     too narrow for the dual-pane layout.
   - Sidebar hidden/shown state preserves the active tab and selection.

5. **Overlays**
   - Searchable command palette with selected row, empty result, and execution
     feedback.
   - Select, alert, confirm, and question overlays with explicit focus and cancel
     behavior.
   - Overlay precedence is deterministic: question/confirm > select/alert >
     palette > page input.

6. **Terminal states**
   - Empty session, loading, streaming, blocked/waiting, provider/auth error,
     unavailable changes, terminal-too-small, and graceful shutdown.
   - Resize recomputes layout without losing prompt text, selected tab, selected
     file, selected hunk, or scroll anchor.

## Boundaries and ownership

The existing terminal, cell, buffer, layout, and widget primitives remain the
rendering substrate. UI work is split into small Zig-owned boundaries:

- `App`: route, overlay precedence, event dispatch, and frame scheduling.
- View state: welcome/home/session state and explicit focus.
- Conversation blocks: bounded metadata plus owned text/code/tool payloads.
- Side-panel state: tab, selected row/hunk, and compact change metadata.
- Overlay state: one active overlay with typed selection and cancel result.
- Fixture source: deterministic UI-only scenarios for screenshots and manual
  review; it is replaceable by a future runtime snapshot without changing
  renderers.

Every allocated string has one visible owner. Long-lived message content uses an
explicit arena or bounded storage owned by the session view and released in its
deinitializer. Prompt/edit buffers own their bytes. Temporary render formatting
uses stack buffers where possible. Transcript, tool detail, palette results,
change rows, and diff hunks are bounded or lazily expanded; no unbounded render
allocation is introduced.

The UI consumes typed snapshots/events. It does not create a second core state
store and does not serialize data through JSON, Protobuf, a worker process, or an
IPC bridge.

## Interaction contract

- Global quit and terminal restoration remain fail-safe.
- Overlay handling consumes events before page handling.
- Tab and focus changes are visible in color, underline, cursor, or selection.
- Mouse click focuses the prompt or selects a visible row; wheel scrolls the
  active scroll surface.
- Keyboard behavior is discoverable from the contextual footer and palette.
- Resize is treated as a first-class event; minimum dimensions show a stable
  recovery screen instead of clipped or panicking output.
- Sending, canceling, retrying, and returning home are visually distinct.
- Error messages identify the affected surface and preserve the user draft.

## Approaches considered

### A. Incremental repair of the current Zig TUI (recommended)

Keep the working terminal/buffer/widget primitives and add explicit UI state and
bounded rendering seams. This minimizes regressions in terminal restoration and
memory ownership, makes each screenshot change reviewable, and keeps the core
boundary clean.

### B. Replace the TUI wholesale

This could produce a cleaner visual result quickly, but would discard already
validated terminal and buffer behavior, enlarge the diff, and make regressions
harder to localize. Rejected.

### C. Port the Rust/OpenCode TUI implementation

This may maximize short-term feature count, but violates the requested
architecture, imports the reference system's coupling and failure modes, and
creates an avoidable memory/maintenance burden. Rejected.

## Acceptance evidence

The UI gate is not complete until:

- the Zig build succeeds;
- a PTY scenario captures all states above at a fixed terminal size;
- a second scenario covers a narrow terminal and resize;
- screenshots are visually inspected against the OpenCode reference for layout,
  hierarchy, spacing, focus, and feedback;
- prompt text, selected tab/file/hunk, and overlay state survive resize;
- no screenshot contains `OMITNIX`, clipped overlay content, stale cursor state,
  or unexplained placeholder/demo-only error text;
- a UI-only memory audit shows ownership/deinitialization for every new buffer,
  string, list, arena, and overlay payload;
- the evidence remains independent of the future core/runtime.

No core feature, provider connection, agent loop, or performance optimization is
part of this UI completion gate.
