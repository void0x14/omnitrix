# Omnitrix Zig TUI UI/UX Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Complete the visible Omnitrix TUI flow in native Zig, matching the OpenCode reference's spatial UX while keeping the implementation, state ownership, and failure behavior Omnitrix-native.

**Architecture:** Repair and extend the existing zero-dependency terminal, cell-buffer, layout, widget, view, and overlay layers. Add typed UI state and bounded ownership seams so deterministic screenshot fixtures can later be replaced by runtime snapshots without changing renderers. Core/runtime/provider integration is excluded until every UI acceptance gate passes.

**Tech Stack:** Zig, the existing raw-terminal backend in `zig/src/omnitrix-tui/core/terminal.zig`, the existing double-buffer renderer, explicit allocators, bounded arrays/arenas, and the existing PTY capture helper with ephemeral `uv` dependencies.

## Global Constraints

- The UI gate must be complete before any runtime/core integration begins.
- Omnitrix will reproduce the visible spatial language and interaction rhythm of the OpenCode TUI while keeping its implementation native to Zig.
- No OpenCode/Rust state machine, renderer, provider contract, permission model, worker process, IPC, or memory ownership model is copied.
- The product name is `OMNITRIX`; `OMITNIX` must not appear in any UI or screenshot.
- The UI consumes typed snapshots/events and does not serialize data through JSON, Protobuf, a worker process, or an IPC bridge.
- Every allocated string has one visible owner; every new buffer, list, arena, and overlay payload has an explicit deinitializer.
- Transcript, tool detail, palette results, change rows, and diff hunks are bounded or lazily expanded; no unbounded render allocation is introduced.
- UI work remains full Zig. Do not restore deleted Rust/core files or add a runtime bridge.
- Preserve the existing dirty worktree. Stage only files belonging to the current task.
- TDD is explicitly disabled by the user. Verification uses targeted Zig builds, PTY scenarios, screenshot inspection, and memory/ownership review; do not add a test-first workflow.
- The current visual reference set is `zig/tools/shots/opencode-ref/`; it is evidence for layout and interaction only, not an implementation source.
- Do not add provider/auth/core behavior; authentication states are UI fixtures with no credential persistence.

---

### Task 1: Typed UI state, overlay precedence, and minimum terminal layout

**Files:**
- Create: `zig/src/omnitrix-tui/core/ui_state.zig`
- Modify: `zig/src/omnitrix-tui/app.zig`
- Modify: `zig/src/omnitrix-tui/core/layout.zig`
- Modify: `zig/src/omnitrix-tui/core/terminal.zig`
- Modify: `zig/src/omnitrix-tui/widgets/scroll.zig`

**Interfaces:**

- `core/ui_state.zig` produces the following exported types:

```zig
pub const Route = enum { welcome, home, session, too_small };
pub const FocusTarget = enum { page, prompt, conversation, sidebar, overlay };
pub const UiStatus = enum { ready, loading, streaming, blocked, error };

pub const Selection = struct {
    tab: u8 = 0,
    row: ?u16 = null,
    hunk: ?u16 = null,
    scroll_anchor: u32 = 0,
};

pub const Viewport = struct {
    cols: u16,
    rows: u16,

    pub const min_cols: u16 = 80;
    pub const min_rows: u16 = 24;

    pub fn isTooSmall(self: Viewport) bool;
};

pub const UiNotice = struct {
    kind: UiStatus = .ready,
    len: u8 = 0,
    bytes: [160]u8 = undefined,

    pub fn set(self: *UiNotice, kind: UiStatus, text: []const u8) void;
    pub fn text(self: *const UiNotice) []const u8;
};

pub const UiState = struct {
    route: Route = .welcome,
    focus: FocusTarget = .prompt,
    status: UiStatus = .ready,
    viewport: Viewport,
    selection: Selection = .{},
    notice: UiNotice = .{},

    pub fn setViewport(self: *UiState, cols: u16, rows: u16) void;
    pub fn clearNotice(self: *UiState) void;
};
```

- `App` owns one `UiState`; views do not allocate or duplicate route/viewport state.
- `App.renderFrame` renders the `.too_small` recovery screen before any other view when `Viewport.isTooSmall()` is true.
- Overlay dispatch order is exactly: question/confirm, select/alert, palette, then page input.
- `Terminal.updateSize` and resize event handling update the same `UiState.viewport` and preserve `UiState.selection`.
- `Terminal.init` must register cleanup immediately after raw/alternate-screen activation so a fallible later allocation cannot leave the terminal in raw mode.
- `SessionView` scroll viewport dimensions are recomputed from the current content rectangle each frame; they are never kept at a fixed `30x80` when the terminal changes size.

- [ ] **Step 1: Read the current app route/event/render flow and identify the exact existing fields that become `UiState` owners. Do not alter unrelated dirty files.**

- [ ] **Step 2: Add `core/ui_state.zig` with the exact types and fixed-size notice storage above. Use no heap allocation in this file.**

- [ ] **Step 3: Integrate `UiState` into `App`; remove duplicate route/viewport decisions only where the new owner is unambiguous. Keep existing view constructors and terminal restoration intact.**

- [ ] **Step 4: Add the minimum-size renderer and make resize recompute the buffer without clearing `prompt`, `selection`, or active overlay state.**
- [ ] **Step 4a: Guard every renderer dimension calculation with saturating/minimum checks; no unsigned range may be created from a terminal dimension below its required border width or height.**

- [ ] **Step 4b: Update the session scroll viewport from the computed conversation rectangle after every resize and before content-to-screen mapping.**

- [ ] **Step 5: Verify the targeted build.**

Run from `zig/`:

```bash
zig build -Doptimize=Debug
```

Expected: exit code 0 and no new compiler diagnostics.

- [ ] **Step 6: Commit only the Task 1 files.**

```bash
git add zig/src/omnitrix-tui/core/ui_state.zig zig/src/omnitrix-tui/app.zig zig/src/omnitrix-tui/core/layout.zig zig/src/omnitrix-tui/core/terminal.zig zig/src/omnitrix-tui/widgets/scroll.zig
git commit -m "feat(tui): add typed UI state and minimum viewport"
```

---

### Task 2: Welcome/auth states and home composer fidelity

**Files:**
- Create: `zig/src/omnitrix-tui/views/welcome.zig`
- Modify: `zig/src/omnitrix-tui/app.zig`
- Modify: `zig/src/omnitrix-tui/views/home.zig`
- Modify: `zig/src/omnitrix-tui/views/footer.zig`

**Interfaces:**

- `views/welcome.zig` produces a UI-only auth surface:

```zig
pub const AuthState = enum {
    signed_out,
    choosing,
    pending,
    authenticated,
    failed,
};

pub const AuthMethod = enum { browser, device_code, api_key };

pub const WelcomeAction = union(enum) {
    none,
    choose_method: AuthMethod,
    begin_auth: AuthMethod,
    cancel,
    complete,
    retry,
};

pub const WelcomeView = struct {
    auth_state: AuthState = .signed_out,
    selected_method: u8 = 0,
    notice_len: u8 = 0,
    notice: [160]u8 = undefined,

    pub fn init() WelcomeView;
    pub fn deinit(self: *WelcomeView) void;
    pub fn render(self: *const WelcomeView, buf: *Buffer, width: u16, height: u16, theme: Theme) void;
    pub fn handleKey(self: *WelcomeView, key: KeyEvent) WelcomeAction;
    pub fn setState(self: *WelcomeView, state: AuthState, notice: []const u8) void;
};
```

- `WelcomeView` never accepts or stores a credential value. The API-key row is a visual method choice with masked placeholder text only.
- The visible brand is exactly `OMNITRIX`, using the established logo/placement style.
- `HomeView` keeps its owned `TextareaWidget` and adds explicit composer mode/focus state for normal, multiline, shell, history, and error feedback.
- The home surface must include the compact mode/model/provider/effort row and path/version footer visible in the reference; the large Omnitrix mark remains branded but must not consume the entire usable hierarchy.
- Any advertised shell or agent affordance must have a working UI-only state transition; otherwise it is removed from the hint row until a later UI task implements it.
- `FooterView` renders contextual hints from the active route, focus, auth state, and composer mode; it must not hard-code a single static hint row.

- [ ] **Step 1: Add `WelcomeView` with deterministic keyboard actions for method choice, pending, cancel, retry, and fixture completion. Keep all text in fixed buffers or static literals.**

- [ ] **Step 2: Route `App` through welcome/auth state without adding provider calls. The fixture completion action transitions to `.home`; cancel and failure remain on the welcome surface with visible feedback.**

- [ ] **Step 3: Update `HomeView.render` and its input path so the prompt, cursor, shell/multiline mode, history affordance, and error notice are visually distinct at the same terminal dimensions as the reference.**
- [ ] **Step 3a: Make the cursor draw beside the placeholder rather than overwriting its first character, and use word-aware wrapping for the home/session prompt text.**

- [ ] **Step 4: Update the footer to show only valid contextual controls for the current route/focus. Ensure `OMITNIX` is absent from source strings touched by this task.**

- [ ] **Step 5: Verify the targeted build.**

```bash
zig build -Doptimize=Debug
```

Expected: exit code 0.

- [ ] **Step 6: Commit only the Task 2 files.**

```bash
git add zig/src/omnitrix-tui/views/welcome.zig zig/src/omnitrix-tui/app.zig zig/src/omnitrix-tui/views/home.zig zig/src/omnitrix-tui/views/footer.zig
git commit -m "feat(tui): complete welcome auth and home composer states"
```

---

### Task 3: Conversation blocks, streaming safe-tail, and prompt focus behavior

**Files:**
- Create: `zig/src/omnitrix-tui/views/conversation.zig`
- Modify: `zig/src/omnitrix-tui/views/session.zig`
- Modify: `zig/src/omnitrix-tui/widgets/markdown.zig`
- Modify: `zig/src/omnitrix-tui/widgets/textarea.zig`
- Modify: `zig/src/omnitrix-tui/input/gap_buffer.zig`
- Modify: `zig/src/omnitrix-tui/app.zig`

**Interfaces:**

- `views/conversation.zig` owns bounded visual blocks:

```zig
pub const BlockKind = enum {
    text,
    thinking,
    tool_use,
    tool_result,
    code,
    error,
};

pub const Block = struct {
    id: u32,
    kind: BlockKind,
    text: []const u8,
    committed_len: usize,
    collapsed: bool = false,
    is_streaming: bool = false,
};

pub const BlockStore = struct {
    arena: std.heap.ArenaAllocator,
    blocks: std.ArrayList(Block),
    max_blocks: usize = 512,

    pub fn init(parent: std.mem.Allocator) BlockStore;
    pub fn deinit(self: *BlockStore) void;
    pub fn append(self: *BlockStore, block: Block) !void;
    pub fn toggleCollapsed(self: *BlockStore, id: u32) bool;
    pub fn safeText(block: Block) []const u8;
};
```

- `safeText` returns the committed prefix for a streaming block and the full text otherwise.
- Message text uses the existing `MarkdownRenderer` path instead of displaying raw `**markers**` and backticks; wrapping must prefer word boundaries and ellipsize only where the visible surface has a hard width cap.
- `SessionView` renders blocks through `BlockStore`; tool blocks show a one-line summary when collapsed and bounded detail when expanded.
- `SessionView` exposes a typed action result:

```zig
pub const SessionAction = union(enum) {
    none,
    send,
    cancel,
    focus_prompt,
    focus_conversation,
    toggle_block: u32,
};
```

- Prompt handling preserves drafts on cancel/error, makes focus visible, and keeps Enter/Shift+Enter behavior deterministic.
- Prompt snapshots have an explicit output-capacity contract. Rendering never copies an unbounded gap buffer into a fixed `[512]` or `[2048]` array.
- Cursor movement, deletion, and word movement operate on UTF-8 codepoint boundaries so Turkish and other multibyte input cannot split a character.

- [ ] **Step 1: Add `BlockStore` with arena ownership, a bounded block count, and safe-tail rendering. All appended text must be copied into the store's arena before the caller can release it.**

- [ ] **Step 2: Move the visual block rendering responsibility out of the monolithic session render path into `conversation.zig` without changing the existing public `SessionView.init/deinit/render` contract.**

- [ ] **Step 3: Add collapsed/expanded tool input/result rows, thinking rows, error rows, and a streaming spinner/status row. Render only `Block.safeText` for streaming content.**

- [ ] **Step 4: Add explicit session/prompt focus routing for keyboard, mouse click, scroll wheel, page up/down, cancel, send, and block toggle. Overlay events must still be consumed first.**
- [ ] **Step 4a: Replace byte-based left/right/backspace assumptions in `gap_buffer.zig` with codepoint-boundary movement and bounded prompt snapshots; preserve the existing explicit allocator ownership.**

- [ ] **Step 4b: Route normal text blocks through `MarkdownRenderer` and fix placeholder cursor placement plus word-aware wrapping.**

- [ ] **Step 5: Verify the targeted build and run the existing full PTY scenario once to catch event-loop regressions.**

```bash
zig build -Doptimize=Debug
uv run --with pyte --with pillow python tools/tui_capture.py --bin zig-out/bin/omnitrix --scenario tools/scenario-omnitrix-full.json --out /tmp/omnitrix-task-3 --cols 120 --rows 32
```

Expected: build exit code 0 and the scenario exits without a non-zero binary status.

- [ ] **Step 6: Commit only the Task 3 files.**

```bash
git add zig/src/omnitrix-tui/views/conversation.zig zig/src/omnitrix-tui/views/session.zig zig/src/omnitrix-tui/widgets/markdown.zig zig/src/omnitrix-tui/widgets/textarea.zig zig/src/omnitrix-tui/input/gap_buffer.zig zig/src/omnitrix-tui/app.zig
git commit -m "feat(tui): add bounded conversation blocks and safe streaming"
```

---

### Task 4: Changes/Diff/sidebar, overlays, and narrow-terminal fallback

**Files:**
- Create: `zig/src/omnitrix-tui/views/changes.zig`
- Create: `zig/src/omnitrix-tui/views/diff.zig`
- Modify: `zig/src/omnitrix-tui/views/session.zig`
- Modify: `zig/src/omnitrix-tui/views/sidebar.zig`
- Modify: `zig/src/omnitrix-tui/dialogs/command_palette.zig`
- Modify: `zig/src/omnitrix-tui/dialogs/modal.zig`
- Modify: `zig/src/omnitrix-tui/app.zig`

**Interfaces:**

```zig
pub const ChangeStatus = enum { added, modified, deleted, renamed, binary, conflict, unavailable };

pub const ChangeRow = struct {
    status: ChangeStatus,
    path: []const u8,
    additions: u32 = 0,
    deletions: u32 = 0,
    actor: []const u8 = "",
};

pub const DiffState = struct {
    selected_file: ?u16 = null,
    selected_hunk: ?u16 = null,
    fullscreen: bool = false,
    unavailable: bool = false,
};
```

- Changes/Diff views receive bounded rows and do not scan the filesystem or Git.
- `SessionView` retains selected tab/file/hunk in `UiState.selection` when the sidebar is hidden or the terminal resizes.
- Diff switches to fullscreen when the viewport cannot display the conversation and side surface without clipping.
- `CommandPalette` exposes an empty-result state and execution notice; it never leaves the dim overlay active after cancellation.
- `ModalDialog` supports select, alert, confirm, and question kinds with typed cancel/confirm results.
- `App.handleEvent` follows the global overlay precedence from Task 1.
- Overlay geometry is clamped to the active session content rectangle when a sidebar is visible; cancellation restores the underlying frame without dimming or stale cursor artifacts.
- Tab labels remain truthful: Changes and Diff render an explicit empty/preview/unavailable surface rather than silently leaving Conversation content visible.

- [ ] **Step 1: Add bounded `ChangeRow` and `DiffState` models with explicit ownership rules for incoming path/actor strings.**

- [ ] **Step 2: Add Changes and Diff renderers with status colors, additions/deletions, actor labels, selected row/hunk feedback, and an unavailable state that is not shown as `0/0`.**

- [ ] **Step 3: Integrate sidebar tab selection and hidden/shown behavior with `UiState.selection`; preserve selected values across resize.**

- [ ] **Step 4: Complete palette empty-result/execution feedback and modal question/confirm/cancel behavior. Verify canceled overlays restore the underlying frame exactly once.**

- [ ] **Step 5: Add the narrow-terminal recovery and fullscreen diff branch; ensure no render loop performs an invalid unsigned range when dimensions are below minimum.**

- [ ] **Step 6: Verify the targeted build and a fixed-size PTY smoke scenario.**

```bash
zig build -Doptimize=Debug
uv run --with pyte --with pillow python tools/tui_capture.py --bin zig-out/bin/omnitrix --scenario tools/scenario-omnitrix-full.json --out /tmp/omnitrix-task-4 --cols 120 --rows 32
```

Expected: exit code 0 for both commands.

- [ ] **Step 7: Commit only the Task 4 files.**

```bash
git add zig/src/omnitrix-tui/views/changes.zig zig/src/omnitrix-tui/views/diff.zig zig/src/omnitrix-tui/views/session.zig zig/src/omnitrix-tui/views/sidebar.zig zig/src/omnitrix-tui/dialogs/command_palette.zig zig/src/omnitrix-tui/dialogs/modal.zig zig/src/omnitrix-tui/app.zig
git commit -m "feat(tui): complete changes diff and overlay states"
```

---

### Task 5: End-to-end screenshot scenarios and visual/memory gate

**Files:**
- Create: `zig/tools/scenario-omnitrix-ui-complete.json`
- Create: `zig/tools/scenario-omnitrix-narrow.json`
- Do not modify generated screenshots or unrelated existing files

**Interfaces and evidence:**

- The complete scenario must capture named frames for welcome signed-out, auth method choice, auth pending, auth failure, authenticated home, home typing, session ready, streaming, collapsed tool, expanded tool, Changes, Diff, palette empty-result, palette filtered, question/confirm modal, sidebar hidden, and session prompt after send.
- The narrow scenario must capture minimum-size recovery, fullscreen diff, and post-resize state with selected tab/file/hunk preserved.
- Capture command:

```bash
zig build -Doptimize=Debug
uv run --with pyte --with pillow python zig/tools/tui_capture.py --bin zig/zig-out/bin/omnitrix --scenario zig/tools/scenario-omnitrix-ui-complete.json --out /tmp/omnitrix-ui-complete --cols 120 --rows 32
uv run --with pyte --with pillow python zig/tools/tui_capture.py --bin zig/zig-out/bin/omnitrix --scenario zig/tools/scenario-omnitrix-narrow.json --out /tmp/omnitrix-ui-narrow --cols 72 --rows 20
```

- [ ] **Step 1: Build the binary and write the complete fixed-size scenario using only supported key/mouse escape sequences.**

- [ ] **Step 2: Write the narrow/resize scenario using the existing PTY helper's `--cols` and `--rows` window-size controls; do not add a runtime dependency to the Zig binary.**

- [ ] **Step 3: Run both captures and confirm every expected PNG exists, the binary exits with status 0, and no capture helper dependency is installed globally.**

- [ ] **Step 4: Visually inspect every generated frame with the OpenCode reference frames. Record any mismatch in spacing, hierarchy, focus, cursor, clipping, stale overlay, or brand text before accepting the task.**
- [ ] **Step 4a: Inspect the home/session frames for placeholder overwrite, raw Markdown, word splitting, clipped sidebar paths, overlay/sidebar competition, and vertical logo/border leakage; correct each mismatch before the visual gate is marked complete.**

- [ ] **Step 5: Perform a source-level memory audit over every new `ArrayList`, arena, fixed buffer, owned slice, and overlay payload; confirm each has one owner and a reachable `deinit`.**
- [ ] **Step 5a: Confirm the source-level audit covers `GapBuffer` cursor boundaries, prompt snapshot capacities, `errdefer` terminal cleanup, scroll viewport resizing, and overlay restoration—not only new allocations.**

- [ ] **Step 6: Run the final targeted build and both screenshot commands again after any visual correction.**

- [ ] **Step 7: Commit only the scenario/helper changes.**

```bash
git add zig/tools/scenario-omnitrix-ui-complete.json zig/tools/scenario-omnitrix-narrow.json zig/tools/tui_capture.py
git commit -m "chore(tui): add complete visual acceptance scenarios"
```

## Final UI gate

Before any core/runtime work starts, verify all of the following against current files and generated frames:

- Welcome/auth, home, session, streaming, tool, Changes, Diff, palette, modal,
  sidebar, narrow, resize, empty, blocked, and error states are visible.
- `OMNITRIX` is correct everywhere and `OMITNIX` has zero matches in UI sources
  and screenshot text.
- OpenCode visual hierarchy is matched, but no OpenCode/Rust architecture or IPC
  is present.
- Prompt draft, tab, file, hunk, scroll anchor, and overlay state survive resize.
- UTF-8 cursor/backspace behavior is safe for Turkish and other multibyte text, with no split codepoints or fixed-buffer overflow.
- No visible hint promises a shell, agent, tab, modal result, or status transition that the current UI cannot actually perform.
- Terminal restoration remains valid on normal exit, Ctrl+C, signal/panic path,
  and minimum-size recovery.
- All new allocations have bounded lifetime and explicit deinitialization.
- Only after this gate passes may a separate core/runtime design begin.
