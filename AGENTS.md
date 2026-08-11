# Repository Guidelines

## Project Structure & Module Organization

`terminal-groove` is a single Rust 2024 binary package. The executable entry point is `src/main.rs`; reusable behavior is exported through `src/lib.rs`.

- `src/model.rs`: project schema, bounded values, defaults, and validation
- `src/reducer.rs`: editing commands, undo/redo, and dirty-state behavior
- `src/persistence.rs`: strict JSON loading and atomic saves
- `src/engine.rs`: transport, sequencing, and step scheduling
- `src/dsp.rs`: oscillators, envelopes, filters, delay, and safety utilities
- `src/audio.rs`: CPAL device integration and real-time command handling
- `src/tui.rs`: Ratatui rendering, keyboard input, and terminal lifecycle

The authoritative behavior is documented in `SPEC.md`. Unit tests currently live beside their implementation in `#[cfg(test)]` modules. Build output belongs in `target/` and must not be committed.

## Build, Test, and Development Commands

- `cargo run --release -- --audio-device null`: run safely with ALSA's null output on Linux.
- `cargo run --release -- --list-audio-devices`: list exact device names accepted by the CLI.
- `cargo test`: run all unit, persistence, and documentation tests.
- `cargo fmt --all -- --check`: verify standard Rust formatting.
- `cargo clippy --all-targets -- -D warnings`: enforce lint-clean code.
- `cargo build --release`: produce the optimized binary at `target/release/terminal-groove`.

On Linux, install `libasound2-dev` on Debian/Ubuntu or `alsa-lib-devel` on Fedora before building. macOS uses CoreAudio and needs no separate audio development package.

## Coding Style & Naming Conventions

Use `rustfmt` defaults and keep the code warning-free. Follow Rust conventions: `snake_case` for modules, functions, and tests; `CamelCase` for types and enum variants; `SCREAMING_SNAKE_CASE` for constants. Keep the model, reducer, and DSP independent of Ratatui and CPAL. Audio callbacks must not allocate, block, lock, access files, or format messages.

## TUI Controls & Dialogs

The terminal UI is split by responsibility. Put persistent screen controls and their display formatting in `src/tui/render.rs`, reusable popup controls and popup geometry in `src/tui/overlays.rs`, keyboard dispatch and mode-specific editing in `src/tui/input.rs`, and project/audio synchronization or file operations in `src/tui/controller.rs`. Keep Ratatui types out of `model.rs` and `reducer.rs`.

### Creating a control

When adding a control, follow the complete path from model value to visible interaction:

1. Define the bounded value, enum, option ordering, default, and validation in `src/model.rs`. Use the model's checked constructors and ranges; do not duplicate limits only in the UI.
2. Add a stable `ParameterId`/`GlobalParameterId` or an explicit editor field as appropriate. For track parameter cards, add a `ParameterDescriptor` with its label, shortcut, and `ParameterGroup` in `src/tui/render.rs`.
3. Add the keyboard behavior in the relevant mode handler in `src/tui/input.rs`. Arrow changes must clamp at the first/last valid value rather than wrap unless the specification explicitly says otherwise. Use Shift for the documented coarse increment and number-row percentage entry only for controls that support it.
4. Apply project changes through `Editor::edit`/the reducer, check the audio command queue before committing, and call `sync_project` (or its smoothing variant) so UI and audio cannot diverge. Transport, cursor, mode, and status changes are not undoable; project edits are.
5. Render the value from the current model in `render.rs` or `overlays.rs`; never maintain a second UI-only value. Add focused tests for lower and upper bounds, disabled/inapplicable values, and the resulting reducer command where practical.

Use the existing control vocabulary:

- Continuous percentage controls are ten vertically stacked segments using `fader_segments`. Show the exact percentage and physical units or derived value when applicable. Direct percentage entry should use the existing short ramp/smoothing behavior.
- Two-position controls are vertical switches rendered with `render_lfo_switch`; show both labels, a selected/unselected marker, and no color-only distinction.
- Ordered discrete controls are selectors rendered with `render_lfo_selector`. Supply choices in the same order used by the model and keyboard handler, fill the available rows, and stop at the ends.
- Track parameter cards use the existing `ParameterDescriptor`/group colors, centered labels, shortcut, BASE/LOCK origin, and `~` LFO marker. Use a double border, reverse styling, yellow/bold emphasis for the active card. LOCK mode must visibly identify effective values and whether they come from a lock or BASE.
- Chord waveform/chorus/spread-style discrete values should retain the established vertical card geometry rather than introducing a new widget shape. The Pitch LFO card is LFO-only and must not pretend to have a BASE or LOCK value.
- For every control, communicate state with text, symbols, or borders as well as color. Disabled controls use muted styling and remain visibly present.

### Dialog guidelines

- Add a `Mode` variant and any cursor/field state in `src/tui/state.rs`. Handle the mode early in `handle_key` in `src/tui/input.rs` so modal keys cannot leak into navigation. Add the matching render branch in `draw_with_device` and a dedicated renderer in `overlays.rs` when the dialog has more than a simple message.
- Use `Clear` for the dialog rectangle, a bordered `Block` with a concise title, and a dedicated `Rect` helper. Center compact dialogs and size them to their content; cap editor dialogs instead of expanding them on large terminals. Keep the underlying project screen visible when that helps context.
- Editors with several related fields use equal-width cards and a field enum with a fixed `ALL` ordering. Left/right selects a field; Up/Down changes it. The active card uses the standard double border/reverse/bold treatment. Use the same control renderer for switches, selectors, and faders as the main UI.
- Show the current value, units/derived value, source or trigger origin where relevant, and visibly muted inactive fields. Keep labels in card borders for compact editors and avoid duplicate labels or empty padding.
- Every dialog must show its local key hints in a short footer or status line. At minimum define Enter (confirm/close), Esc (cancel/close or the documented immediate-edit behavior), and any destructive action. Help text must match the actual handler.
- Text-input dialogs must preserve literal user input, show the resolved path before file confirmation, reject empty/invalid input visibly, and define Backspace behavior. Confirmation dialogs use the consistent `Save [S]`, `Discard [D]`, `Cancel [Esc]` wording.
- Modal edits must have an explicit commit policy. Immediate arrow edits keep their changes on Esc where the specification says so; destructive operations require confirmation; failed reducer/audio operations leave the project unchanged and report an actionable error.
- Respect small terminals: use the existing minimum-size fallback and saturating `Rect` calculations. Do not panic when a popup is narrower or shorter than expected; render an empty-safe result or the fallback screen.
- Update `SPEC.md` for new modes, controls, shortcuts, sizing, or commit semantics, and add rendering/input tests when behavior is non-trivial. Manually verify dialogs at the documented `120x34` size and at a smaller terminal size.

## Testing Guidelines

Add focused tests in the module being changed. Name tests after observable behavior, such as `wrapped_tie` or `fractional_clock_has_no_drift`. Cover valid boundaries and rejection paths, especially JSON ranges, tie graphs, timing, and non-finite DSP values. Run formatting, tests, and Clippy before submitting.

## Commit & Pull Request Guidelines

History is small and uses concise, imperative summaries, for example `Initial terminal-groove implementation`. Keep commits scoped to one coherent change. Pull requests should explain user-visible behavior, identify affected `SPEC.md` sections, list verification commands, and include terminal screenshots for layout changes. Document any audio-device-specific manual testing and link relevant issues.
