# SigmaRift TUI UX/UI Guide

This file defines the interaction, visual and structural rules for the SigmaRift Ratatui interface.

The goal is consistency and low implementation ambiguity. A new screen should be able to follow this file without inventing new navigation, focus, colors, overlay behavior or repeated Ratatui rendering patterns.

This is a product contract, not only a color reference. UX behavior and UI styling are intentionally defined together because focus, selection, overlays and keyboard behavior are part of the same system.

---

## 1. Locked Product Model

The following decisions are intentional and should not be changed casually.

### 1.1 Startup model selection

- Model selection is a startup concern, not a Control-menu setting.
- If exactly one usable model is available, skip `MODEL SELECT` and enter the main UI directly.
- If more than one usable model is available, open `MODEL SELECT` before the main UI.
- The selected model is fixed for the running session/runtime. Do not expose model switching inside an active conversation.
- Do not add model selection to `CONTROL`.
- If no usable model exists, show a blocking startup error state rather than an empty selector.

### 1.2 Main UI versus Control

The main UI is for frequent work:

- session list;
- current session transcript;
- message input;
- runtime status;
- creating/selecting/deleting sessions.

Attachments/multimodal are intentionally deferred during the recovery cycle. Do not expose placeholder attachment actions until the model/application path is implemented end-to-end.

`CONTROL` is the single gateway for implemented infrequent configuration and application-level actions, for example:

- LLM Parameters;
- Additional Instructions;
- Profiles;
- Quit.

Do not promote rarely used configuration into permanent main-screen panels.

Do not render placeholder or no-op Control rows. `LLM PARAMETERS`, Additional Instructions,
Profiles are implemented end-to-end. Attachments/multimodal and other deferred
features stay absent until their schema and end-to-end Application operation are
implemented.

### 1.3 Control shortcut

`Ctrl+Space` opens `CONTROL` from the main UI.

Do not use `Alt` shortcuts for the primary Control action. Terminal/SSH modifier behavior is not reliable enough to make them the UX contract.

`Ctrl+Space` is not used to stack another Control menu on top of an existing modal/editor. The topmost overlay owns input until it is closed or completed.

---

## 2. Core UX Principles

1. The interface is keyboard-only. Every SigmaRift action and navigation path must be usable without a mouse.
2. Mouse interaction is not part of the application UX contract. Do not implement click, hover, wheel, drag-and-drop or mouse-only affordances. Terminal-native paste may still be used.
3. `Tab` means focus. It does not perform a screen-specific action.
4. Arrow keys operate inside the currently focused region.
5. Only one main-screen region is focused at a time.
6. A modal/overlay traps input; the background remains visible but inactive.
7. `Esc` means back, close or cancel inside nested UI. On the main work surface, it exits SigmaRift.
8. The current keyboard context must always be visible in the local `SHORTCUTS` box when space permits.
9. Frequently used actions stay on the main screen; infrequent configuration belongs in `CONTROL`.
10. The same conceptual action must have the same key and visual treatment everywhere.
11. Do not depend on laptop-unfriendly keys such as `PgUp`/`PgDn` as the only way to navigate content.

---

## 3. Main Screen Focus Model

The focus order is fixed:

```text
SESSIONS -> SESSION -> INPUT -> SESSIONS
```

- `Tab` = focus next.
- `Shift+Tab` = focus previous.
- `RUNTIME`, logo and `SHORTCUTS` are informational and are never part of the Tab cycle.
- Focus state must survive redraws and content updates.
- Opening an overlay temporarily removes focus from the main screen. Closing it restores the previous main-screen focus.

### 3.1 `SESSIONS` focus

Primary behavior:

- `↑` / `↓` = move the session cursor and immediately activate the hovered stored session.
- `Enter` = activate the `+ NEW SESSION` action row.
- `Delete` may delete the selected session, but destructive actions must use the shared confirmation pattern if accidental activation would cause data loss.

The currently active session and navigation cursor are separate states. Moving the cursor onto a stored session synchronizes the active session; the `+ NEW SESSION` cursor position remains an action only.

`+ NEW SESSION` is an action, not a session item:

- keep it visually separated from stored sessions;
- render it above stored sessions; stored sessions begin below its separating blank row;
- use a filled low-intensity action treatment based on the existing Raised / Input Background rather than the normal session-row treatment;
- use `+` as the explicit action marker;
- when the cursor is on the row, the normal Active Green cursor highlight takes precedence;
- do not give it a persistent-current-session marker.
- activating it creates and selects the new session, then moves main focus to `INPUT`.

For stored session rows:

```text
❏ Current session
❏ Other session
❏ Another session
```

- `❏` is the consistent marker for every stored session; the active session retains an Active Green background with Main Background text even when `SESSIONS` is unfocused;
- cursor highlight takes precedence while navigating; moving it onto a stored session activates that session immediately.
- `❏` marks every stored session and inherits its row foreground, including the inverted active-row treatment. Stored rows remain compact without blank spacer rows.
- when stored sessions exceed the available list area, the list scrolls its rows to keep the cursor-visible active session in view. Do not render a session-list scrollbar.
- the first non-empty user message names a new session from its first non-empty line, truncated to 48 characters with `...` when needed; later messages do not rename it.

### 3.2 `SESSION` focus

Primary behavior:

- `↑` / `↓` = scroll transcript by a small step.
- `PgUp` / `PgDn` = optional page scroll accelerator.
- `End` jumps to the latest transcript output and resumes follow behavior.
- `Home` may jump to the start when implemented.

The transcript must never require `PgUp`/`PgDn` for basic navigation because many laptop keyboards hide those keys behind `Fn` combinations.

When new output arrives:

- if the user is already at/near the bottom, continue following the newest content;
- if the user has manually scrolled upward, do not force-jump them to the bottom;
- provide an obvious way to return to the newest content through normal scrolling/end behavior.

### 3.3 `INPUT` focus

Primary behavior:

- normal typing edits the message;
- arrow keys are editor/cursor movement;
- `Enter` = send;
- `LeftAlt+Enter` = insert newline. Right Alt may act as AltGr and is not a portable terminal binding.
- `Tab` leaves the input and moves focus; it must not insert a tab character in chat mode.

Do not overload `↑` / `↓` with prompt history while they are needed for multiline cursor movement. If prompt history is added later, give it an explicit separate binding.

---

## 4. Overlay and Navigation Model

All modal screens use the same behavior.

### 4.1 Overlay rules

- The topmost overlay owns keyboard input.
- Main-screen focus is visually de-emphasized while the overlay is open.
- The overlay uses the shared thin `overlay(...)` composition around native Ratatui layout, `Clear`, and panel primitives.
- Modal sizing policy, padding, border/title styling and parent/child composition must not be reimplemented per screen.
- A child editor/selector opened from `CONTROL` returns to `CONTROL` when completed or cancelled unless the action explicitly exits the application.
- The full modal footprint is an opaque Raised / Input Background support surface. It includes the space between stacked overlay panels; terminal-default or background content must never show through it.
- Overlay panels inherit that same Raised background. A modal is one monolithic lighter surface against the main background; do not add a darker panel layer or a separate outer support-surface apron.
- Overlay panels use one terminal cell of inner padding on every side. This keeps content one cell away from vertical borders and leaves one blank content row between a title border and the first selectable or editable row.
- Selectable menu rows are the exception to horizontal panel padding: the cursor background spans the full inner row width between the panel borders, while its text has one blank cell on both sides.
- When a selectable label exceeds its available row width, truncate the label with `...`; never let it overwrite the border or silently wrap into another menu row.

### 4.2 `Esc`

`Esc` is contextual:

- startup `MODEL SELECT`: quit/cancel startup;
- `CONTROL`: close Control and return to the main UI;
- editor: discard/cancel edits and return to the parent screen;
- nested selector/dialog: close/cancel and return to the parent screen.

On the main screen, `Esc` exits SigmaRift. The application has no destructive unsaved UI transaction at this level, and the same binding is reliable in local terminals, SSH, and Windows terminals. Inside an overlay/editor/dialog, `Esc` always closes or cancels the topmost overlay first.

### 4.3 Control menu

`CONTROL` uses:

- `↑` / `↓` = move cursor;
- letter accelerator = immediately open or activate the matching action;
- `Enter` = open/activate the selected action;
- `Esc` = close Control.

Letter accelerators and arrow navigation are complementary. The letter key
immediately activates its matching row; arrow navigation keeps every action
discoverable without memorization.

The first implementation contains these rows, in order:

```text
LLM Parameters              M
Additional Instructions     I
Profiles                    F
Quit                        Q
```

`LLM PARAMETERS` persists runtime-wide settings in `sessions/runtime.json`.
The primary view contains Temperature, Max tokens, Top P, Top K, Min P and
Repeat penalty. `Advanced...` expands in the same overlay; optional advanced
values, including stop sequences and grammar, display `Default` and are omitted
from the model request until set.
These are runtime settings, never session fields.

`ADDITIONAL INSTRUCTIONS` edits `prompts/instructions.md` through the shared
multiline editor. `Ctrl+S` saves and `Esc` discards. This optional file is
appended after `prompts/system.md`; the base tool, workspace, and playbook
instructions remain manually editable only from `prompts/system.md`.

`PROFILES` lists Markdown filenames from `prompts/profiles/` as a multi-select
list. A session persists selected profile names only; each selected file is
read in that stored order before the next model request. Missing selected
profile files cause a request failure rather than silently changing the
session's prompt.

Reasoning is always rendered when supplied by the model. It is part of the
local model's visible work state and is not a user preference.

### 4.4 Text editor overlays

Long-form editors such as `ADDITIONAL INSTRUCTIONS` use the shared multiline editor implementation.

Editor mode:

- arrow keys = move cursor;
- `Enter` = newline;
- `PgUp` / `PgDn` = optional page scroll;
- `Ctrl+S` = save/commit;
- `Esc` = discard/cancel;
- cursor remains visible through automatic viewport scrolling.

Chat input and long-form editor must share the same underlying editor implementation where practical, but may use different keymaps.

## 5. Shortcut Presentation

Shortcut hints are part of the interaction contract, not decorative help text.

### 5.1 Formatting

Use the shared format:

```text
[ Key ]        Action
```

For arrow keys, use glyphs rather than words:

```text
[ ↑ ] [ ↓ ]    Navigate
```

Do not render:

```text
[ Up ] [ Down ]
```

Use `←`, `→`, `↑`, `↓` consistently when arrow keys are shown.

Within one shortcut box, every action label begins in the same column. The shared shortcut helper reserves and pads a fixed key column rather than spacing each row independently. The key column must be compact enough that the longest contextual action label fits in the available panel width; shorten the key column before truncating an action label.

Every shortcut box uses the same presentation: System Blue bold key labels, Muted Text action labels, and the shared aligned key/action columns. Main-screen and overlay shortcut boxes may differ in their available actions, never in this visual treatment.

### 5.2 Context

- A shortcut box shows actions available in the current context.
- Do not show bindings that are disabled in the current focus/overlay.
- Main-screen shortcut help may include global bindings plus the behavior of the currently focused region.
- Overlay shortcut help describes the overlay, not the obscured main screen.
- `SHORTCUTS` is informational and uses System Blue regardless of which interactive element is focused.

---

## 6. Core UI Principles

1. Prefer native Ratatui widgets and layout primitives.
2. Add custom helpers only for repeated SigmaRift patterns.
3. Do not build a second UI framework on top of Ratatui.
4. Repeated visual and interaction behavior must be centralized.
5. Screens describe composition and screen-specific data; helpers own shared presentation rules.
6. Focus, cursor selection, persistent active state, information and execution state must remain visually distinct.
7. New screens may use native Ratatui widgets directly when no reusable SigmaRift pattern exists.
8. Color is semantic. Never assign a color merely because it “looks good” on one screen.
9. The UI remains compact, terminal-native and information-dense. It must not imitate a desktop GUI.


### 6.1 UI ownership boundary

The production UI is a view/controller over `Control` state. It is **not** a second owner of sessions.

The UI may own only presentation state, for example:

```rust
struct UiState {
    focus: MainFocus,
    overlay: Option<OverlayState>,
    session_cursor: SessionCursor,
    input: TextEditor,
    transcript_scroll: TranscriptScroll,
}
```

The exact fields may evolve, but the ownership rule is fixed:

- UI may keep session IDs, cursors, focus, editor buffers, scroll positions and overlay-local draft values;
- UI must not keep authoritative mutable `Session` clones;
- UI must not persist sessions directly;
- UI must not reconstruct domain state from stale local copies;
- session list identity is its stable string ID, never vector index;
- cursor position and active session remain separate concepts.

`Control` exposes renderable state through snapshots/views, conceptually:

```rust
struct AppSnapshot {
    sessions: Vec<SessionSummary>,
    active_session: Option<SessionView>,
}
```

A snapshot is read-only from the UI's perspective.

### 6.2 Direct Control boundary

UI intent crosses the `Control` boundary through direct semantic methods, not a command bus and not mutated domain objects.

Conceptually:

```rust
control.create_session()
control.select_session(&id)
control.delete_session(&id)
control.toggle_active_session_profile(name)
control.submit(content)
control.interrupt()
control.poll()
control.shutdown()
```

Do not reintroduce commands such as:

```rust
SaveSession(Session)
StartTurn { session: Session, ... }
```

`Control` owns the one active turn. UI state identifies sessions by stable ID and renders fresh snapshots after each poll. Late events from a cancelled stream are ignored before they can change persistent state.

---

## 7. Semantic Color Palette

| Role | HEX | RGB | Usage |
|---|---|---:|---|
| Main background | `#091013` | `9, 16, 19` | Application background; dark text on bright inverted labels |
| Panel background | `#0D191D` | `13, 25, 29` | Standard panel and transcript surface |
| Raised / input background | `#122328` | `18, 35, 40` | Input row, raised areas, logo surface, overlay support surface |
| Editor / soft highlight background | `#162F36` | `22, 47, 54` | Text editor surface and low-intensity highlight; not the primary menu cursor color |
| Active green | `#53E0C2` | `83, 224, 194` | Focus border/title, MODEL badge, active menu cursor |
| Processing teal | `#4F9B94` | `79, 155, 148` | Temporary `PROCESSING...` model-work state |
| System blue | `#5DB1FF` | `93, 177, 255` | YOU, shortcuts, help, runtime/system information |
| Logo blue | `#0080FF` | `0, 128, 255` | Sigma logo accent only |
| Reasoning purple | `#AF97FF` | `175, 151, 255` | `REASONING` section label |
| Unfocused border | `#2B464B` | `43, 70, 75` | Inactive interactive panel border/title line |
| Muted text | `#7E999B` | `126, 153, 155` | Secondary text, hints, reasoning body |
| Primary text | `#DDEBE8` | `221, 235, 232` | Main readable model/content text |
| Tool yellow | `#E8B860` | `232, 184, 96` | `EXEC`, command/tool activity |
| Tool dim yellow | `#BF9750` | `191, 151, 80` | `EXEC RESULT`, secondary tool/technical literals |
| Error red | `#FF6F6F` | `255, 111, 111` | Errors and failed-operation text |
| Error accent | `#BE6868` | `190, 104, 104` | Error overlay border and title background |

### 7.1 Color rules

- Active Green is reserved for focus/action/active-model semantics. Do not use it on passive panels merely to make them prominent.
- Processing Teal is reserved for the temporary `PROCESSING...` model-work badge.
- System Blue is for informational/system/user-help semantics.
- Reasoning Purple is reserved for the reasoning section header, not generic accents.
- Tool Yellow is for execution/tool semantics only.
- Error Red is for actual errors/failures only.
- Error Accent is reserved for error overlay borders and titles.
- Warning is intentionally not defined yet. Do not invent a warning color locally; add one here and in `theme.rs` when the product needs a formal warning state.
- Do not introduce screen-local RGB/HEX literals.

---

## 8. Focus, Selection and Active-State Styling

These are different states and must not be collapsed into one generic “selected” style.

### 8.1 Focused interactive panel

A focused panel:

- border = Active Green;
- title background = Active Green;
- title foreground = Main Background;
- title remains integrated into the top border, not rendered as a detached floating pill;
- receives keyboard input.

Conceptually:

```text
┌─ INPUT ───────────────────────────────┐
```

The ` INPUT ` title span itself is inverted. Include minimal horizontal padding inside the styled title span so the filled background reads as a rectangle touching the border line.

### 8.2 Unfocused interactive panel

An unfocused panel:

- border = Unfocused Border;
- title uses the unfocused treatment without a bright filled background;
- content remains readable and unchanged;
- must not look disabled unless it actually is disabled.

`SESSION` is the deliberate exception: when unfocused, it uses the System Blue informational border/title treatment because it remains the primary work surface. It becomes Active Green when it owns scroll/navigation input. `SESSIONS` and `INPUT` use the normal Unfocused Border treatment until focused.

### 8.3 Active overlay

The active overlay uses the same title rule as a focused panel:

- Active Green border;
- inverted Active Green title;
- dark title text.

Examples:

- `CONTROL`
- `ADDITIONAL INSTRUCTIONS`
- `PROFILE`
- `MODEL SELECT`

There is one visual language for “this currently owns input”.

Error overlays are the deliberate exception: they use Error Accent for the
border and inverted title so a blocking failure remains distinct from an
actionable overlay.

### 8.4 Cursor row in actionable menus/selectors

The currently navigated row in `CONTROL`, `MODEL SELECT` and similar menus:

- background = Active Green;
- foreground = Main Background;
- must be obvious without relying on a symbol alone.

This is intentionally stronger than the Editor / Soft Highlight background.

### 8.5 Persistent active state

Persistent state is separate from cursor position.

Do not use a generic paired marker such as `◆ / ◇` for every list. Use a persistent marker only when the state needs to remain visible independently of the cursor.

For the session list, use one compact lower-right drop-shadowed white-square marker for every stored session:

```text
❏ current
❏ other
```

The current row retains an Active Green background and Main Background text when the panel is unfocused. Inactive stored rows use the same `❏` marker without that persistent color treatment.

Moving the cursor onto a stored session immediately changes the active session. Landing on `+ NEW SESSION` never creates a session; `Enter` remains its explicit action.

### 8.6 Informational panels

Non-interactive informational areas such as `RUNTIME`:

- are not in the focus cycle;
- use System Blue for their informational title/border treatment, or the explicitly defined informational panel style;
- must not use the Focused Green title treatment because that would falsely imply keyboard ownership.

---

## 9. Session Transcript Visual Language

The transcript uses stable semantic shapes and colors. Do not redesign these per message type.

### 9.1 User message

`YOU` is a filled badge:

- background = System Blue;
- foreground = Main Background;
- user message body may use System Blue where the current transcript design does so.

Shape stays compact and rectangular as in the existing UI.

### 9.2 Model message

`MODEL` is a filled badge:

- background = Active Green;
- foreground = Main Background;
- normal model answer body = Primary Text.

### 9.3 Reasoning

`REASONING` is a filled badge under `MODEL`, aligned with other model work
events such as `EXEC` and `EXEC RESULT`:

```text
   REASONING
```

- badge background = Reasoning Purple;
- badge foreground = Main Background;
- body = Muted Text;
- body = italic where supported by the current terminal/font rendering;
- reasoning remains visually subordinate to the final answer.

During streaming, reasoning and final-answer deltas append only to their own live sections. `REASONING` must never be relabeled as thinking, assistant, or generic model prose.

### 9.3.1 Streaming transcript order

- render the user message first as `YOU`;
- render a live `MODEL` section for final-answer deltas;
- render an indented `REASONING` badge and body only when the server supplies
  reasoning deltas;
- persist the completed reasoning and final answer as distinct transcript data;
- follow the newest output while the transcript remains at its bottom; preserve a manual upward scroll position.

Use the same indented badge composition as other model-work events.

### 9.4 Tool execution

`EXEC` is a filled badge:

- background = Tool Yellow;
- foreground = Main Background.

The command/tool invocation line uses Tool Yellow.

### 9.5 Tool result

`EXEC RESULT` is a filled badge using the dimmer tool family:

- background = Tool Dim Yellow;
- foreground = Main Background.

Tool result body normally returns to Primary Text unless a specific structured element has a defined tool style.

While the model is actively processing input before it emits its next reasoning
or final-answer delta, render an indented `PROCESSING...` badge:

```text
   PROCESSING...
```

- the badge uses the Processing Teal treatment;
- it is presentation-only state, never a persisted message;
- it appears after a submitted user message or `EXEC RESULT` and disappears when the next MODEL delta arrives;
- when the active turn has not yet produced any MODEL output, render its normal `MODEL` badge, one blank row, then `PROCESSING...`;
- do not render an empty second `MODEL` badge for this waiting interval.

### 9.6 Errors

Errors use Error Red.

Do not automatically force every error into a badge. Existing inline/runtime/model error text may remain plain red text when that matches the event structure.

### 9.7 Inline technical values

Inline code, command fragments, ports, addresses and similar technical literals may use Tool Dim Yellow when semantic highlighting improves scanning.

Do not color arbitrary prose words as technical values.

### 9.8 Markdown presentation

User, MODEL and REASONING bodies render their stored Markdown only in the
`SESSION` view; persistence retains the original text. User content is always
rendered verbatim. A complete MODEL response wrapped in a ` ```markdown ` or
` ```md ` fence may have only that outer wrapper removed for presentation;
nested fenced code blocks remain part of the document.

- headings are bold body text without a visible `#` marker;
- unordered and ordered lists, blockquotes, emphasis, links and tables use
  the Markdown renderer's native Ratatui line composition;
- links use System Blue with an underline;
- table borders use Muted Text and table headers are bold;
- inline and fenced code use Tool Dim Yellow without visible fence delimiter
  rows;
- syntax-specific code colors are intentionally absent until they can follow
  the semantic palette.

---

## 10. Main Layout Contract

The main application is a persistent shell.

```text
┌ LEFT RAIL ┐  ┌ SESSION ───────────────────────────────┐
│ Logo      │  │                                        │
│ Runtime   │  │ Transcript                             │
│ Sessions  │  │                                        │
│ Shortcuts │  └────────────────────────────────────────┘
└───────────┘  ┌ INPUT ─────────────────────────────────┐
               │ Message editor                         │
               └────────────────────────────────────────┘
```

Rules:

- left rail remains structurally stable while sessions change;
- the brand area and `RUNTIME` share the same panel background and have no separator row between them; the brand includes the compact Muted Text label `LLM-DRIVEN SYSTEM OPERATIONS`;
- `RUNTIME` starts after one blank row below the brand label;
- `SESSION` receives the majority of expandable space;
- `INPUT` stays compact and grows only as required by the chosen multiline-input behavior;
- the main input is an eight-row panel with one-cell inner margins and a four-row textarea viewport. The textarea scrolls its own viewport as needed.
- the input field has one terminal cell of horizontal padding on both sides inside its panel.
- transcript and input are visually separate regions;
- the transcript shows a scrollbar only when it scrolls; the session list scrolls its rows without a scrollbar;
- no screen should hardcode pixel assumptions; use Ratatui character-cell layout constraints.

### 10.1 Runtime token presentation

When token/context usage is available, present it inside the non-interactive `RUNTIME` area rather than creating a new focusable panel. Keep it compact and informational.

A suitable shape is:

```text
CONTEXT   2,148 / 8,192
REQUEST   1,932
OUTPUT      216
LEFT      6,044
```

Rules:

- use System Blue / normal runtime informational styling;
- do not make token counters focusable or interactive unless a future explicit feature requires it;
- prefer exact values over decorative gauges;
- if space is constrained, prioritize `CONTEXT used / limit` and `LEFT`;
- do not invent separate colors for token states.

---

## 11. Reusable Ratatui Patterns

Shared UI code must stay thin. It should encode SigmaRift's repeated styling and interaction rules while leaving layout and rendering to native Ratatui primitives.

Do **not** recreate Ratatui concepts such as `Block`, `Rect`, `Clear`, `ListState`, `TableState`, `Paragraph` or `ScrollbarState` behind compatibility wrappers.

### `panel(...)`

Ratatui basis:

- `Block`, `Borders`, `Paragraph` and `Layout`;
- styled `Span` and `Line` values for title/content.

Responsibilities:

- focused/unfocused/informational border treatment;
- integrated inverted focused title;
- standard panel background/padding where applicable.

It should be a thin composition helper, not a retained widget object with its own hidden state.

### `overlay(...)`

Ratatui basis:

- `Clear`, `Block`, `Layout` and a shared opaque background.

Responsibilities:

- consistent modal size policy;
- active border/title treatment;
- consistent inner padding;
- centering, clearing, and painting the opaque Raised / Input Background support surface;
- parent/child overlay composition.

Use the shared `components::overlay(...)` helper for centering, clearing and painting the support surface.

### `menu_rows(...)`

Ratatui basis:

- `List`, `ListState`, `Paragraph` and styled explicit rows.

Responsibilities:

- Active Green cursor row;
- optional semantic persistent marker;
- accelerator/status column alignment where needed;
- separation of cursor position from persistent active state.

Menu navigation state lives in screen/UI state, not in a hidden list-state widget.

### `shortcut_box(...)`

Ratatui basis:

- informational `Block`, `Paragraph`, `Line` and `Span` composition.

Responsibilities:

- consistent `[ Key ]  Action` layout;
- arrow glyph rendering (`↑ ↓ ← →`);
- System Blue styling;
- context-aware rows.

### `text_editor(...)`

Preferred implementation:

- shared `TextEditor` adapter;
- native `tui_textarea::TextArea` state and widget.

Use one shared editor adapter for multiline editable text.

Modes:

- chat mode: `Enter = Send`, `LeftAlt+Enter = New line`, `Tab = leave focus`;
- editor mode: `Enter = New line`, `Ctrl+S = Save`, `Esc = Discard/Cancel`.

The adapter must respect SigmaRift semantic focus. Rendering a native textarea must **not** automatically grant it application-level input ownership. Only route unconsumed editor events to `TextEditor` when that editor is the active semantic input target.

### `transcript(...)`

Ratatui basis:

- `Paragraph`, `Wrap`, `Scrollbar` and `tui_markdown` output;
- styled `Line` and `Span` values for badges, reasoning headers, exec events and errors.

Responsibilities:

- transcript ordering;
- semantic message/event styling;
- scroll offset / follow-bottom behavior;
- preserving manual scroll position while streaming.
- reasoning is rendered as a visually invisible nested flow with a stable three-cell indent; every wrapped continuation line retains that indent and stays within the transcript content width.

Do not build a second markdown renderer or hide Ratatui paragraph/scrollbar behavior behind a parallel widget layer.

### `badge(...)`

Ratatui basis:

- styled `Span`.

Supported semantic variants:

- `YOU`;
- `MODEL`;
- `EXEC`;
- `EXEC RESULT`.
- `PROCESSING...`.

`REASONING` uses the model-work badge treatment.

### `selectable_row(...)`

Ratatui basis:

- one explicit `ListItem` or `Paragraph` row composition.

Responsibilities:

- Active Green cursor highlight;
- a consistent `❏` session marker; persistent active state is shown by the row treatment;
- optional low-intensity action treatment such as `+ NEW SESSION`;
- dark foreground on bright cursor background.

### `status_value(...)`

Ratatui basis:

- styled `Span` using semantic theme constants.

Only globally defined status meanings receive semantic colors.

---

## 12. Preferred Native Ratatui Mapping

| UI element | Preferred implementation |
|---|---|
| Screen layout | `Layout::horizontal` / `Layout::vertical` |
| Panel | `Block`, `Borders` and `Paragraph` |
| Logo | styled `Span` and `Line` |
| Runtime fields | compact `Line` rows |
| Session list | `List` with `ListState`; cursor stored by stable ID |
| Shortcut box | informational `Block` and `Paragraph` |
| Session transcript | `Paragraph`, `Wrap`, `Scrollbar`, `tui_markdown` and styled event rows |
| Model selector | centered `Clear`/`Block` composition and explicit rows |
| Control menu | centered `Clear`/`Block` composition and explicit rows |
| Confirmation dialog | centered `Clear`/`Block` composition and explicit action rows |
| Semantic badges | styled `Span` |
| Reasoning header | styled `Span` |
| Editable multiline text | `TextEditor` + native `tui_textarea::TextArea` |

Prefer direct native composition. A SigmaRift helper is justified only when it centralizes a repeated semantic rule; it must not hide Ratatui behind a parallel widget API.

---

## 13. Production UI Structure

The production UI is split by responsibility inside `src/tui/`.

```text
src/tui/
  mod.rs          terminal + top-level UI lifecycle
  state.rs        UI-only state and stable identities
  input.rs        raw key -> semantic UiAction mapping
  render.rs       screen composition and Ratatui rendering
  components.rs   shared overlay composition
  theme.rs        palette + semantic style helpers
  text_editor.rs  shared tui-textarea adapter
```

Add another screen file only when the screen exists as a real implemented feature. Do not create placeholder files for deferred features.

### `mod.rs`

Owns only the top-level UI loop/lifecycle:

- receive terminal events;
- request `Control` snapshots and runtime state;
- run semantic routing;
- request redraw;
- dispatch the active screen composition;
- initiate shutdown through `Control::shutdown`.

The current implementation also routes Control overlays here; rendering remains in `render.rs`.

### `state.rs`

Contains UI-only state, for example:

- `MainFocus`;
- overlay/screen stack state;
- `SessionCursor` stable-ID cursor identity;
- editor buffers / `TextEditor`;
- transcript scroll/follow state;
- local confirmation/dialog state;

It must not own authoritative mutable `Session` domain objects.

### `input.rs`

Maps raw terminal input to semantic UI actions.

Examples:

```text
FocusNext
FocusPrevious
MoveUp
MoveDown
Activate
Delete
Back
OpenControl
Interrupt
```

A semantic action is either consumed by the current UI context or intentionally left for the focused native editor. Literal key matching for shared behavior must not be duplicated across screens.

### `components.rs`

Contains the shared Ratatui overlay composition helper:

```text
overlay(...)
```

No domain persistence, `Control` calls or session mutation belongs here.

### `theme.rs`

Contains:

- palette constants;
- semantic style constructors;
- active/inactive/informational panel styles;
- cursor styles;
- transcript semantic styles;
- tool/reasoning/error styles.

No screen state or routing belongs here.

### `render.rs`

Owns main-screen and overlay composition using native Ratatui widgets. It consumes `AppSnapshot` and `RuntimeView`; it does not persist or mutate sessions directly.

---

## 14. Input and Event Routing

Input ownership is explicit and ordered. This is required to avoid key leakage into native `TextEditor` state and to ensure overlays trap input correctly.

### 14.1 Terminal key routing

Conceptually:

```text
raw terminal key
      ↓
normalize/map to semantic UiAction when applicable
      ↓
[topmost overlay?] ── yes ──> overlay handles/consumes action
      │
      no
      ↓
resolve main/global action
      ↓
route to focused main region
      ↓
if still unconsumed AND focused target is an editor
      ↓
forward raw editor event to TextEditor
```

Rules:

- topmost overlay has first ownership;
- `Esc` in an overlay closes/cancels that overlay and must not cancel a background model turn;
- `Ctrl+Space` is consumed when it opens `CONTROL`; the same raw event must not insert a space into a textarea;
- chat `Enter` is consumed when it becomes `Send`; it must not also create a newline;
- `Tab` / `Shift+Tab` are consumed by SigmaRift focus navigation where defined and must not reach the textarea;
- a raw event that produced a consumed semantic action is not forwarded again to native widgets;
- inactive/native-rendered textareas receive no editing events merely because they are present in the tree.

### 14.2 Main-screen `Esc` and cancellation

When **no overlay is open**, `Esc` exits SigmaRift. Turn cancellation, when implemented, receives its own explicit binding rather than overloading the universal back/exit key.

When an overlay/editor/dialog is open, its `Esc` behavior always wins over turn cancellation.

### 14.3 Control polling and snapshots

The UI polls `Control`, then renders a fresh `AppSnapshot` and `RuntimeView`.

Rules:

- never identify sessions by vector index;
- deleting/reordering sessions cannot redirect an in-flight turn;
- cancelled model streams remain pending until their worker closes, and late events are ignored;
- a failed turn refreshes the authoritative session state rather than retaining a stale UI copy;
- UI does not “save back” an `AppSnapshot`.

### 14.4 Submit gating

While a turn is active for the relevant runtime/session:

- a second submit is blocked;
- the input may remain editable unless product behavior explicitly chooses otherwise;
- `Control` is the source of truth for whether a submit is accepted.

Do not solve double-submit by cloning or locking a UI-owned `Session`.

---

## 15. Screen Responsibilities

TUI code should mainly:

1. compose its native Ratatui layout;
2. read summaries and the active historical session through `Control` snapshots;
3. read/update UI-only state;
4. render through semantic theme/components;
5. translate semantic UI actions into local transitions or direct `Control` calls.

A screen should not:

- own or clone authoritative `Session` state;
- persist session JSON;
- send generic `SaveSession(Session)` actions;
- identify sessions by `usize`/vector position;
- hardcode palette values;
- rebuild modal styling/spacing locally;
- invent new focus styling;
- invent a new shortcut layout;
- implement its own multiline editor behavior;
- duplicate reusable selection logic;
- reinterpret consumed keys inside native widgets;
- use Active Green for a passive panel.

---

## 16. Abstraction Rule

Create a shared component/helper when:

- the same semantic presentation appears in at least two places; or
- consistency is important enough that divergence would be a bug.

Keep helpers shallow. They should compose Ratatui primitives and styles rather than creating a new retained widget hierarchy.

Good:

```text
overlay(...)
TextEditor
```

Usually unnecessary:

```text
sigma_text(...)
sigma_container(...)
sigma_row(...)
```

unless they later acquire real SigmaRift-specific semantics.

If a helper mostly renames one Ratatui call, remove it and use the native call directly.

---

## 17. Agent Implementation and Test Rules

When implementing or changing the TUI, an agent:

- MUST preserve the locked UX model in sections 1-5 unless explicitly asked to change it.
- MUST keep the application keyboard-only; mouse interaction must not be required or introduced as an application control path.
- MUST treat UI state as presentation state only; authoritative session mutation stays behind the `Control` boundary.
- MUST identify sessions with stable IDs, never vector index.
- MUST NOT reintroduce `SaveSession(Session)` or pass mutable `Session` clones across the UI boundary.
- MUST route topmost overlay input before main-screen/global cancellation logic.
- MUST consume semantic action keys before forwarding input to `TextEditor`.
- MUST ensure an unfocused textarea cannot mutate merely because it is rendered.
- MUST block a second submit while a turn is active.
- MUST reuse existing shared components when the required semantic pattern already exists.
- MUST use colors/styles from `theme.rs`.
- MUST preserve the semantic color meanings defined here.
- MUST render active panel/overlay titles with the shared inverted Active Green title treatment.
- MUST keep only `SESSIONS`, `SESSION` and `INPUT` in the main Tab focus cycle unless the product design explicitly changes.
- MUST use arrow glyphs in shortcut labels instead of `Up`/`Down` text.
- MUST keep model selection out of `CONTROL`.
- MUST NOT add live model switching to an active conversation.
- MUST NOT expose placeholder/no-op rows such as `LLM PARAMETERS` before they are implemented end-to-end.
- MUST render `+ NEW SESSION` as a distinct action row rather than as a normal stored-session row.
- MUST use a consistent `❏` marker for stored sessions; distinguish the active session with its persistent row treatment rather than a filled marker.
- MUST route infrequent implemented configuration through `CONTROL` rather than adding permanent main-screen controls.
- MUST prefer native Ratatui primitives and `tui_markdown` rendering.
- MUST use the shared `TextEditor` adapter for multiline editable text.
- MUST NOT copy a component's rendering logic into a screen.
- MUST NOT introduce arbitrary new colors.
- MUST NOT create a parallel UI abstraction layer without an explicit architectural reason.
- MUST keep focus, cursor selection, persistent active state, informational state and execution state visually distinct.
- SHOULD keep screen code declarative and composition-oriented.
- SHOULD add a helper only when repeated semantics justify it.

### 17.1 Required production UI boundary tests

Use Ratatui's test backend/event facilities where practical. Tests should exercise state transitions and input ownership, not only whether a raw key can be observed.

At minimum cover:

- `Tab` / `Shift+Tab` focus cycle: `SESSIONS -> SESSION -> INPUT`;
- topmost overlay traps `Esc` and does not cancel an active background turn;
- `Ctrl+Space` opens `CONTROL` without leaking a space into chat input;
- chat `Enter` submits once and does not also insert a newline;
- `LeftAlt+Enter` inserts a newline without submitting;
- session cursor remains correct across session insertion/deletion by stable ID;
- deleting another session cannot redirect an in-flight turn;
- second submit is blocked while a turn is active;
- failed/cancelled turn refreshes authoritative session state;
- closing an overlay restores the previous main-screen focus.

Tests should target semantic state transitions first. Screenshot/golden rendering tests may be added for stable visual contracts, but they do not replace routing/ownership tests.

---

## 18. Current Design Intent

SigmaRift should feel like a coherent keyboard-driven terminal instrument rather than a collection of unrelated screens.

The intended mental model is:

```text
start app
   ↓
MODEL SELECT only when needed
   ↓
main work surface
   ├── Tab changes focus
   ├── arrows operate focused region
   └── Ctrl+Space opens CONTROL
                         ↓
                  configuration/action
                         ↓
                  editor/selector/dialog
```

Visual hierarchy comes primarily from:

- semantic colors;
- active-title inversion;
- borders;
- compact badges;
- background contrast;
- spacing;
- concise labels;
- stable keyboard behavior.

New UI should fit this system instead of adding decorative complexity or new navigation concepts.
