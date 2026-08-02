# terminal-groove

A Linux-first, real-time terminal groovebox. It provides six independently looping 1–64-step sequences, 909-inspired procedural drums, a 303-inspired monophonic Bass track, two flexible monophonic synths, strict versioned JSON projects, undo/redo, parameter locks, and per-parameter LFOs. Track sequences can be resized or doubled up to 64 steps.

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
The current strict schema uses `format_version: 5`; versions 1–4 are rejected without migration. Drum and note events carry a Boolean accent, while Bass notes can also arm a fixed-time slide into the next note. Eligible continuous parameters can each have an independent track-level LFO, opened from the parameter editor with `Shift+L`.

The output stage uses fixed +6 dB makeup gain and a stereo-linked lookahead limiter at -1 dBFS, giving patterns competitive playback level without adding a master-volume setting.
