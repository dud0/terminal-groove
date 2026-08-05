# terminal-groove

A Linux-first, real-time terminal groovebox. It provides six independently looping 1–64-step sequences, 909-inspired procedural drums, a 303-inspired monophonic Bass, a six-voice Juno-60-inspired Chord synth, an SH-101-inspired monophonic Lead, strict versioned JSON projects, undo/redo, parameter locks, and per-parameter LFOs. Chord sequencer degrees produce ascending diatonic 1–3–5 triads.

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
The current strict schema uses `format_version: 11`; versions 1–10 and unknown versions are rejected without migration. Top-level tracks contain shared configuration only; every sequence belongs to its pattern. Projects contain 1–100 dynamic patterns. Open the horizontal pattern dialog with `Ctrl+P`; Left/Right, Home, and End move its cursor, and `Enter` selects the cursor pattern while stopped or queues it while playing. Insert, duplicate, copy, cut, paste, and delete operations are also available there. Drum and note events carry a Boolean accent, while Bass notes can also arm a fixed-time slide into the next note. Chord shape and arpeggio settings belong to individual Chord note triggers; ties inherit them and empty Chord steps edit insertion defaults. Eligible continuous parameters—including Chord/Lead oscillator mix, pulse width, and sub level—can each have an independent track-level LFO, opened from the parameter editor with `Shift+L`.

The output stage uses fixed +6 dB makeup gain and a stereo-linked lookahead limiter at -1 dBFS, giving patterns competitive playback level without adding a master-volume setting.
