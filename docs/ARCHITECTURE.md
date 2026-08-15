# Architecture and Maintenance

`terminal-groove` separates persisted musical state, reversible editing, terminal interaction,
sequencing, and real-time rendering. `SPEC.md` is authoritative for user-visible behavior; this
document records implementation boundaries and maintenance rules.

## Data flow

The TUI reads `Project` through `Editor`. Mutations go through scoped reducer helpers, which record
region-based undo deltas, revision identities, audio edit impacts, and structural pattern-index
changes. Before committing an edit, the controller checks command-queue capacity. A main-thread
snapshot builder then reuses unchanged `Arc`-backed audio patterns and sends an immutable snapshot
plus any pattern/song-index mappings through a bounded SPSC queue.

The callback owns voices, sequencing state, smoothers, and effects. Each callback processes at most
eight commands and coalesces only adjacent identity-map replacements. Structural replacements and
intervening commands retain FIFO order. Replaced and superseded snapshots go to a retirement queue
sized to the command queue and are reaped on every UI iteration. Status returns through atomics;
stream errors use a bounded queue and are logged outside the callback.

Live recording taps each final limited stereo frame before CPAL channel mapping. The callback sends
frames and an ordered end marker through a preallocated two-second SPSC queue to a named WAV writer
thread. That worker blocks on its command receiver while idle, then performs sample conversion,
disk I/O, periodic header checkpointing, and final header updates while a take exists. The UI
creates a unique destination and prepares the encoder before issuing the allocation-free callback
start command. Completion or failure returns as an event containing the path and accepted frame
count; recording state itself is transport-independent.

## Layer boundaries

- `model`: persisted types, bounds, compatibility, defaults, and validation.
- `reducer`: mutation, clipboards, dirty state, coalescing, and undo/redo.
- `persistence`: strict versioned JSON loading and atomic saves.
- `engine`: transport-independent timing and gate decisions.
- `dsp`: allocation-free signal-processing primitives.
- `audio`: CPAL integration, snapshots, scheduling, voices, and rendering.
- `audio::recording`: bounded live-master capture and asynchronous 24-bit WAV writing.
- `tui`: Ratatui rendering, modes, input, file operations, and synchronization.

Song references are intentionally persisted and maintained during structural pattern edits before
song playback and editing are exposed. Preserve their validation, undo behavior, and remapping.
Project JSON is first parsed by a recursive duplicate-key-rejecting visitor, before migrations or
typed deserialization, so duplicate fields cannot be hidden by `serde_json::Value` normalization.

## Control metadata

`ParameterId` owns parameter count, storage index, wire name, value kind, bounds, track
compatibility, lock eligibility, and LFO eligibility. TUI descriptors add presentation and ordering
only. Global labels, shortcuts, and ordering live in the TUI's single global-control catalog;
numeric bounds and mutation do not.

When adding a parameter, update model policy and access first, then the relevant TUI descriptor,
readout and input path, reducer tests, and audio application. Keep Ratatui and CPAL types out of the
model and reducer.

## Real-time constraints

The callback must not allocate, destroy project snapshots, block, lock, access files, or format
messages. Keep live and audition state independent, while sharing plain allocation-free routing
calculations where sidechain, mute, and tail semantics remain explicit. Prefer fixed arrays,
bounded queues, cached mappings, and control-rate coefficient updates.

Recording never performs file access, allocation, formatting, waits, or sample conversion in the
callback. One capture-queue slot is reserved for its end marker; exhaustion ends the take instead
of dropping a frame, so any retained audio is a contiguous prefix. Stream shutdown drops the
callback producer before the writer's final drain and joins the writer thread.

Allocation safety is automated; timing measurements are host-dependent. See
`AUDIO_PERFORMANCE.md` for the reference fixture and results.

## Verification

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

For callback changes, also run:

```sh
cargo test --release audio::tests::saturated_callback_benchmark -- --ignored --nocapture
```

Record OS, architecture, Rust toolchain, and profile with benchmark results rather than adding
build scripts solely to inject metadata.
