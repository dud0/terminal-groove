# Repository Guidelines

## Project Structure

`terminal-groove` is one Cargo package with a library target (`src/lib.rs`) and
the executable target (`src/main.rs`). It uses Rust 2024 and supports Linux and
macOS. `SPEC.md` is authoritative for user-visible behavior, keyboard input,
the JSON schema, and acceptance criteria. `docs/ARCHITECTURE.md` is authoritative
for implementation boundaries and real-time ownership; `docs/AUDIO_PERFORMANCE.md`
contains the callback benchmark method and evidence.

Build output belongs in `target/` and must not be committed. Legacy working-
directory storage is ignored and must not be reintroduced or imported.

## Build and Verification

- `cargo test`: run unit, persistence, audio, TUI, and documentation tests
- `cargo fmt --all -- --check`: verify standard Rust formatting
- `cargo clippy --all-targets -- -D warnings`: enforce lint-clean code
- `cargo build --release`: build `target/release/terminal-groove`
- `cargo run --release -- --audio-device null`: run with ALSA's null output when that device is available
- `cargo run --release -- --list-audio-devices`: list exact output-device names
- `cargo run --release -- --audio-device <exact-name> --audio-buffer <frames>`: test explicit device and buffer selection
- `cargo test --release audio::tests::saturated_callback_benchmark -- --ignored --nocapture`: run the host-dependent callback benchmark

The CLI accepts an optional project path, uses the default output device unless
overridden, and rejects unsupported or ambiguous explicit devices and buffer
sizes. Do not assume that the `null` device exists on every platform.

## Code and Real-Time Rules

Use rustfmt defaults, Rust naming conventions, and warning-free code. Keep the
model, reducer, and DSP independent of Ratatui and CPAL. Put file I/O and
formatting on the main/UI or worker threads, never in the audio callback.

The audio callback must not allocate, deallocate, block, lock, access the
filesystem, format messages, or log. It may only use preallocated state and
bounded lock-free communication. In particular:

- The main thread owns terminal input, rendering, file I/O, undo/redo, and the canonical `Project`.
- Project edits become immutable audio snapshots and are sent through the bounded SPSC command queue.
- Unchanged audio patterns should be structurally shared where the edit impact permits it.
- The callback processes at most eight commands per callback and preserves ordering around structural or intervening commands.
- Replaced snapshots go through the retirement queue and are reclaimed outside the callback.
- Live playback and audition state remain independent, including voices, effects, LFOs, and effect tails.
- CPAL errors, non-finite diagnostics, and callback telemetry leave the callback through bounded or atomic status paths and are reported outside it.
- Recording captures the final limited internal stereo pair through a preallocated queue. The callback only queues frames and an ordered end marker; the named worker thread performs WAV I/O, 24-bit conversion, flushing, and finalization.

## Persistence and Storage

Project files are strict, pretty-printed UTF-8 JSON ending in a newline. Current
projects save as format v26, reject duplicate keys, unknown fields, invalid
ranges, incompatible events/locks/LFOs, invalid tie graphs, and invalid pattern
or song references. Versions 21 through 25 are migrated as specified in
`SPEC.md`; unsupported and malformed versions must be rejected without changing
the current project or undo history. Saves validate first, write a temporary
sibling, flush and sync it, then atomically rename it.

The normal root is `Terminal Groove/` below the OS Music directory, with
`Projects/`, `Presets/<track-kind>/`, `Recordings/`, and `Logs/` subdirectories.
If no Music directory is available, use the visible home-directory fallback.
Project files use `.groove.json`; user presets use `.preset.json`. Explicit CLI
project paths remain literal. Do not import or modify legacy `.projects/`,
`.presets/`, or `.recordings/` directories.

Track presets are separate sound-only strict JSON. New presets use format v4;
versions 1 through 3 remain loadable through the documented migrations. Loading
a preset must preserve track identity, mute, swing, probability, input defaults,
patterns, steps, and locks. Built-in presets are immutable; default presets
affect new untitled projects only.

## Feature Invariants

- Projects have ten stable track slots, each independently assignable to any documented instrument kind, one through 100 dynamic patterns, and one-based song references to those patterns.
- Sequence data belongs to patterns; each pattern track has one through 64 steps. Structural pattern edits must preserve or correctly rebase active, queued, and song references.
- The pattern-idea generator is session-only, deterministic from its seed, fills empty steps only, and is applied as one undoable project edit.
- Audition voices and effects are independent from live playback. Recording is transport-independent and captures the final limited internal stereo pair until explicitly stopped or the application exits.

## TUI Controls and Dialogs

Keep TUI responsibilities separated by module as described in
`docs/ARCHITECTURE.md`. Keep Ratatui types out of `model.rs` and `reducer.rs`.
For a new control, complete the full path:

1. Define its bounded model value, enum ordering, default, compatibility, and validation in `model.rs`.
2. Add `ParameterId`/`GlobalParameterId` policy in the model as appropriate, then add presentation metadata to `render.rs` or `controls.rs`.
3. Add keyboard behavior in the correct mode handler in `input.rs`; clamp ordered values at their ends unless `SPEC.md` explicitly requires wrapping.
4. Apply project edits through reducer APIs, check queue capacity before committing, synchronize the audio snapshot, and leave UI/audio state unchanged when synchronization fails.
5. Render from the model, not a second UI-only value, and add focused boundary, rejection, input, or rendering tests as appropriate.

For dialogs and overlays:

- Add a `Mode` variant and field state in `state.rs`, dispatch the mode before general navigation in `input.rs`, and add the matching render branch.
- Keep popup geometry safe on small terminals and preserve the documented small-terminal fallback.
- Use fixed field-order arrays or enums for multi-field editors. Left/right selects fields; Up/down changes values unless the specification says otherwise.
- Local key hints must match the actual handler.
- Dirty-project confirmations use `Save [S]`, `Discard [D]`, and `Cancel [Esc]`. Overwrite confirmations use `Overwrite [Enter/O]` and `Cancel [Esc]`; preset-default confirmations use their documented Set/Clear keys.
- Define whether edits are immediate or committed on confirmation. Immediate arrow edits retain their changes on Esc where specified; destructive actions require confirmation; failed reducer, storage, or audio operations leave the project unchanged.
- Preserve literal text input, reject invalid names visibly, and define Backspace behavior.
- Add or update `TestBackend` coverage for the documented `120x34` layout and smaller terminals.

Update `SPEC.md` for new modes, controls, shortcuts, sizing, persistence, or
commit semantics rather than letting this file become a second product spec.

## Testing

Add focused tests beside the implementation. Cover model bounds and rejection,
strict JSON and migrations, duplicate keys, tie graphs, pattern/song rebasing,
undo/redo and dirty revisions, timing and scheduling, finite DSP output, LFO
behavior, effect tails, recording conversion, and callback allocation/deallocation
safety. TUI changes should use `TestBackend` at `120x34`, larger sizes, and a
smaller terminal where relevant. Name tests after observable behavior, such as
`wrapped_tie` or `fractional_clock_has_no_drift`.
