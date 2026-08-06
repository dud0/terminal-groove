# terminal-groove

A Linux-first, real-time terminal groovebox. It provides eight independently looping 1–64-step sequences, 909-inspired procedural drums including Tom and Cymbal voices, a 303-inspired monophonic Bass, a Juno-60-inspired Chord synth with two alternating four-voice groups, an SH-101-inspired monophonic Lead, strict versioned JSON projects, undo/redo, parameter locks, and per-parameter LFOs. Chord sequencer degrees produce diatonic shapes with inversions and optional arpeggiation.

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

Projects are strict, human-readable `.groove.json` files stored by default in `.projects/`, which is ignored by Git. `Ctrl+O` browses every project in that directory; Save As accepts only a project name and adds `.groove.json` automatically. Explicit project paths remain unchanged.
The current strict schema uses `format_version: 16`; unsupported and unknown versions are rejected without migration. Top-level tracks contain shared configuration only; every sequence belongs to its pattern. Projects contain 1–100 dynamic patterns. Open the horizontal pattern dialog with `Ctrl+P`; Left/Right, Home, and End move its cursor, and `Enter` selects the cursor pattern while stopped or queues it while playing. Insert, duplicate, copy, cut, paste, and delete operations are also available there. Drum and note events carry a Boolean accent, while Bass notes can also arm a fixed-time slide into the next note. Chord shape and arpeggio settings belong to individual Chord note triggers; ties inherit them and empty Chord steps edit insertion defaults. Eligible continuous parameters—including Chord/Lead oscillator mix, pulse width, and sub level—can each have an independent track-level LFO, opened from the parameter editor with `Shift+L`. The global `Ducking` card (`d`) provides an optional kick-keyed sidechain compressor for Bass, Chord, and Lead; its depth, attack, and release are edited together and default to Off.

The output stage uses fixed +6 dB makeup gain and a stereo-linked lookahead limiter at -1 dBFS, giving patterns competitive playback level without adding a master-volume setting.
