# terminal-groove

A real-time terminal groovebox for Linux and macOS. It provides ten independently looping 1–64-step sequences, procedural drum voices including Tom, Cymbal, and Rimshot, a monophonic resonant Bass synth, a polyphonic Chord synth with two alternating four-voice groups, a monophonic Lead synth, and a monophonic four-operator FM synth. Projects are strict versioned JSON with undo/redo, parameter locks, and per-parameter LFOs. Chord sequencer degrees produce diatonic shapes with inversions and optional arpeggiation.

## Build prerequisites

Install Rust 1.85 or newer with [rustup](https://rustup.rs/). On Linux, also install the ALSA development headers:

```sh
# Debian / Ubuntu
sudo apt install libasound2-dev

# Fedora
sudo dnf install alsa-lib-devel
```

On macOS, CPAL uses CoreAudio and no separate audio development package is required.

Then build and test:

```sh
cargo test
cargo build --release
```

Run with `cargo run --release`, optionally passing a project path and `--audio-device <exact-name>`. Use `--list-audio-devices` to discover output names. The terminal must be at least 120 columns by 34 rows for the full fader layout.

Projects are strict, human-readable `.groove.json` files stored by default in `Terminal Groove/Projects/` inside your OS Music folder. `Ctrl+O` browses every project in that directory; Save As accepts only a project name and adds `.groove.json` automatically. Presets, recordings, and audio diagnostics are stored alongside it in visible `Presets/`, `Recordings/`, and `Logs/` folders. If the OS has no Music folder, Terminal Groove uses `~/Terminal Groove/`. Existing legacy working-directory folders are left untouched; explicit project paths remain unchanged.
The current strict schema uses `format_version: 22`. Canonical nine-track v21 projects are upgraded on load by appending the default FM track and an empty 16-step FM sequence to every pattern; other versions and malformed projects are rejected. Saves always emit v22. Top-level tracks contain shared configuration only; every sequence belongs to its pattern. Projects contain 1–100 dynamic patterns. FM uses ordinary monophonic notes and ties, without Chord articulation or slide. Eligible continuous parameters—including each FM operator's Level and Feedback, Brightness, ADSR, and Pitch—can each have an independent track-level LFO with optional trigger reset and a configurable starting phase, opened from the parameter editor with `Shift+L`. The global `Ducking` card (`d`) provides an optional kick-keyed sidechain compressor for Bass, Chord, Lead, and FM; its depth, attack, and release are edited together and default to Off.

FM is fixed track 10 (`Shift+0` or `)`). Its parameter bank provides Algorithm (`q`), four compact operator summaries and the operator editor (`Shift+O`), Brightness (`c`), Pitch LFO (`i`), and shared ADSR (`a/d/s/r`). The editor keeps OP1–OP4 visible while editing each sine operator's Ratio, Level, and Feedback. Eight fixed algorithms cover deep stacks, split and converging modulators, paired stacks, shared modulators, mixed carriers, and additive output.

The output stage uses fixed +6 dB makeup gain and a stereo-linked lookahead limiter at -1 dBFS, giving patterns competitive playback level without adding a master-volume setting.

Press `Ctrl+R` from the sequencer or an editor to start a live take, and press it again to stop. Recording is independent of transport and captures the final limited internal stereo master—including auditions, effect tails, pauses, and silence—at the active device sample rate. Takes are stereo 24-bit PCM WAV files in `Terminal Groove/Recordings/` inside your OS Music folder, named `<project>-<unix_timestamp_ms>.wav` (`untitled` for an unsaved project); recording continues until manually stopped or the application exits.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for implementation boundaries and
[`docs/AUDIO_PERFORMANCE.md`](docs/AUDIO_PERFORMANCE.md) for callback benchmark evidence.
