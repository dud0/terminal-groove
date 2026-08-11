# Architecture and Maintenance

`terminal-groove` separates persisted musical state, reversible editing, terminal interaction,
sequencing, and real-time rendering. `SPEC.md` is authoritative for user-visible behavior; this
document records implementation boundaries and maintenance rules.

## Data flow

The TUI reads `Project` through `Editor`. Mutations go through the reducer, which records
region-based undo deltas and structural pattern-index changes. Before committing an edit, the
controller checks command-queue capacity and then sends an immutable audio snapshot plus any
pattern-index mapping through a bounded SPSC queue.

The callback owns voices, sequencing state, smoothers, and effects. Replaced snapshots go to a
retirement queue and are destroyed by the UI thread. Status returns through atomics; stream errors
use a bounded queue and are logged outside the callback.

## Layer boundaries

- `model`: persisted types, bounds, compatibility, defaults, and validation.
- `reducer`: mutation, clipboards, dirty state, coalescing, and undo/redo.
- `persistence`: strict versioned JSON loading and atomic saves.
- `engine`: transport-independent timing and gate decisions.
- `dsp`: allocation-free signal-processing primitives.
- `audio`: CPAL integration, snapshots, scheduling, voices, and rendering.
- `tui`: Ratatui rendering, modes, input, file operations, and synchronization.

Song references are intentionally persisted and maintained during structural pattern edits before
song playback and editing are exposed. Preserve their validation, undo behavior, and remapping.

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
