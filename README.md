# terminal-groove

A Linux-first, real-time terminal groovebox. It provides one 16-step pattern with three synthesized drum tracks and three monophonic synth tracks, strict versioned JSON projects, undo/redo, parameter locks, and procedural audio.

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

Run with `cargo run --release`, optionally passing a project path and `--audio-device <exact-name>`. Use `--list-audio-devices` to discover output names. The terminal must be at least 80 columns by 24 rows.

Projects are strict, human-readable `.groove.json` files. A supplied name is never modified automatically.
