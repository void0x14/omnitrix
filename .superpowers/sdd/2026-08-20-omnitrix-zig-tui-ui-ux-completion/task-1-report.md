# Task 1 Implementer Report — Typed UI State, Overlay Precedence, and Minimum Terminal Layout

## Status

Implemented Task 1 UI-only state, recovery, resize, scroll, and terminal-cleanup ownership work. No core, runtime, provider, or connection path was started or added.

## Changed Files

- Created `zig/src/omnitrix-tui/core/ui_state.zig`
  - Added allocation-free `Route`, `FocusTarget`, `UiStatus`, `Selection`, `Viewport`, `UiNotice`, and `UiState`.
  - `Viewport` declares the required 80-column by 24-row threshold.
  - `UiNotice` has fixed 160-byte storage; it performs no allocation.
- Modified `zig/src/omnitrix-tui/app.zig`
  - Replaced app-owned route storage with one `UiState`.
  - Synchronizes `UiState.viewport` on both polling resize and explicit resize events without resetting selection, prompt, or overlays.
  - Installs `errdefer terminal.deinit()` immediately after `Terminal.init` so later fallible initialization restores raw mode, alternate screen, mouse tracking, cursor, and the buffer.
  - Renders the minimum-terminal recovery screen before pages, modals, palettes, or Kitty output.
  - Recomputes the session conversation rectangle and calls `ScrollContainer.setViewport` before `SessionView.render` can map content to screen coordinates.
  - Makes modal-before-palette-before-page input precedence explicit. The existing ModalDialog is mutually exclusive by dialog type, covering question/prompt or confirm before select or alert.
- Modified `zig/src/omnitrix-tui/core/layout.zig`
  - Added the saturating `sessionLayout` helper shared by App and the session geometry convention.
  - Kept all geometry operations saturating so a zero/small terminal cannot create unsigned underflow ranges.
- Modified `zig/src/omnitrix-tui/widgets/scroll.zig`
  - Added `setViewport(height, width)`; it recomputes viewport ownership each frame and preserves a manual scroll offset by clamping it, while auto-scroll remains at the bottom.
- Inspected but did not modify `zig/src/omnitrix-tui/core/terminal.zig`
  - It already registers raw-mode restoration immediately after `tcsetattr` and before the fallible buffer allocation. The App-level `errdefer` closes the remaining post-initialization gap.

## Design Decisions

- `UiState` is the single owner of route, viewport, focus, status, selection, and notice state. Views receive dimensions only; they do not own duplicate route or viewport state.
- The app checks `Viewport.isTooSmall()` before rendering any existing page or overlay. This avoids invoking current renderers whose border/range calculations assume the 80x24 minimum.
- Session scroll coordinates stay owned by `ScrollContainer`; the App supplies the actual conversation rectangle before content-to-screen mapping. The `ScrollContainer.init(30, 80)` seed therefore cannot persist across a render/resize.
- Existing ModalDialog types are mutually exclusive, so a single modal branch preserves deterministic priority over the palette. There are no parallel question/select/alert instances in this Task 1 UI.

## Memory and Cleanup Ownership

- `UiState` and `UiNotice` use only value fields and fixed-size byte storage; neither allocates or deinitializes.
- App owns and deinitializes HomeView, SessionView, CommandPalette, and Terminal.
- Terminal owns its Buffer and terminal modes. Terminal restores raw mode if its own buffer allocation fails; App restores Terminal if palette initialization fails after raw/alternate-screen activation.
- ScrollContainer owns only scalar viewport/offset state and does not allocate.

## Verification

Command run from `zig/`:

```sh
zig build -Doptimize=Debug
```

Exact output:

```text
<no output>
```

Exit code: `0`.

Additional source/ownership review:

```sh
git diff --check -- zig/src/omnitrix-tui/app.zig zig/src/omnitrix-tui/core/ui_state.zig zig/src/omnitrix-tui/core/layout.zig zig/src/omnitrix-tui/widgets/scroll.zig zig/src/omnitrix-tui/core/terminal.zig
```

Exit code: `0`; no whitespace errors. The review confirmed the recovery gate occurs before page/overlay render calls, both resize paths update the same UiState viewport, and ScrollContainer receives the conversation rectangle before session rendering.

## Self-Review

- Confirmed no runtime, provider, core connection, or non-Zig implementation was introduced.
- Confirmed the page renderers and Kitty overlay are bypassed below 80x24.
- Confirmed modal input is evaluated before palette input and page input.
- Confirmed resize does not assign or clear prompt, selection, modal state, palette state, or scroll anchor.
- Confirmed fixed 30x80 initialization is overwritten from the computed conversation rectangle before content mapping.
- Confirmed the changed Task 1 code is formatted and the targeted Zig build passes.

## Concerns

- The required committed design path was absent from this checkout, so implementation used the supplied Task 1 brief as the binding requirement source.
- The checkout contains extensive unrelated pre-existing changes. In particular, app.zig and terminal.zig were already modified. Only Task 1 hunks are staged for the commit; pre-existing hunks remain unstaged.
- Zig reserves `error` and disallows a `text` parameter shadowing the `text()` method. The semantic enum tag is declared as `@"error"`, and UiNotice.set uses parameter name `value`; these are the minimal compiling representations of the mandated API values and behavior.
