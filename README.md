# terminal-groove

A Linux-first, real-time terminal groovebox. It provides six independently looping 1–64-step sequences with three synthesized drum tracks and three monophonic synth tracks, strict versioned JSON projects, undo/redo, parameter locks, and procedural audio. Track sequences can be resized or doubled up to 64 steps.

## Build prerequisites

Install Rust 1.85 or newer with [rustup](https://rustup.rs/), plus ALSA development headers:

```sh
# Debian / Ubuntu
sudo apt install libasound2-dev

# Fedora
sudo dnf install alsa-lib-devel
```

Then build and test:

```sh
cargo test
cargo build --release
```

Run with `cargo run --release`, optionally passing a project path and `--audio-device <exact-name>`. Use `--list-audio-devices` to discover output names. The terminal must be at least 120 columns by 34 rows for the full fader layout.

Projects are strict, human-readable `.groove.json` files. A supplied name is never modified automatically.
The current strict schema uses `format_version: 3`; earlier project versions are rejected without migration. Eligible parameters can each have an independent track-level LFO, opened from the parameter editor with `Shift+L`.
