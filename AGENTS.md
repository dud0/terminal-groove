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

- `cargo run --release -- --audio-device null`: run safely with ALSA's null output.
- `cargo run --release -- --list-audio-devices`: list exact device names accepted by the CLI.
- `cargo test`: run all unit, persistence, and documentation tests.
- `cargo fmt --all -- --check`: verify standard Rust formatting.
- `cargo clippy --all-targets -- -D warnings`: enforce lint-clean code.
- `cargo build --release`: produce the optimized binary at `target/release/terminal-groove`.

Install `libasound2-dev` on Debian/Ubuntu or `alsa-lib-devel` on Fedora before building.

## Coding Style & Naming Conventions

Use `rustfmt` defaults and keep the code warning-free. Follow Rust conventions: `snake_case` for modules, functions, and tests; `CamelCase` for types and enum variants; `SCREAMING_SNAKE_CASE` for constants. Keep the model, reducer, and DSP independent of Ratatui and CPAL. Audio callbacks must not allocate, block, lock, access files, or format messages.

## Testing Guidelines

Add focused tests in the module being changed. Name tests after observable behavior, such as `wrapped_tie` or `fractional_clock_has_no_drift`. Cover valid boundaries and rejection paths, especially JSON ranges, tie graphs, timing, and non-finite DSP values. Run formatting, tests, and Clippy before submitting.

## Commit & Pull Request Guidelines

History is small and uses concise, imperative summaries, for example `Initial terminal-groove implementation`. Keep commits scoped to one coherent change. Pull requests should explain user-visible behavior, identify affected `SPEC.md` sections, list verification commands, and include terminal screenshots for layout changes. Document any audio-device-specific manual testing and link relevant issues.
