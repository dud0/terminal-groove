# Implementation Plan: Juno-60 Chord and SH-101 Lead Voices

## Goal

Give Chord and Lead distinct, hardware-informed signal paths while sharing only genuinely common low-level primitives.

The two voices should be redesigned together because they currently share the same oscillator/filter pipeline. A joint refactor avoids duplicating primitives while preventing their instrument-specific behavior from collapsing back into one generic synth.

## Findings addressed

- A7: Juno-60 and SH-101 behavior is insufficiently differentiated.

## Shared design principles

- Keep phase-aligned, band-limited oscillators and existing oversampling safety.
- Represent source mixers as additive sources rather than an exclusive pulse/saw crossfade internally.
- Use instrument-specific filter calibration and gain staging.
- Preserve the existing sequencer, locks, LFO assignments, chord spread, arpeggiator, and undo/persistence behavior where possible. The format-17 schema is an intentional breaking change; older project files remain unsupported rather than being migrated.
- Avoid adding controls until the DSP behavior and model migration are explicitly designed.

## Work plan

### 1. Refactor common oscillator primitives

- Expose phase-aligned saw and pulse output without forcing equal-power crossfade semantics.
- Add reusable sub-oscillator modes needed by the Juno and SH-101.
- Add deterministic noise sources local to each voice.
- Ensure all source combinations remain bounded before filter drive.
- Keep oscillator work skippable for zero-level sources when their phase does not need advancement, or advance phase cheaply without generating unused waveforms.

### 2. Define parameter compatibility and migration

- Decide whether the existing `oscillator_mix` control remains as a macro over additive pulse/saw levels or is replaced by independent controls.
- Prefer a backward-compatible macro mapping unless a schema bump and TUI expansion are explicitly accepted. This implementation explicitly accepts the format-17 schema bump and does not migrate older projects.
- Define how existing project values map to the new source levels so saved projects do not change unexpectedly.
- Add model validation, reducer commands, persistence versioning (with no migration), TUI descriptors, locks, and LFO compatibility for any new parameter.

### 3. Implement the Juno-60-inspired Chord path

- Use a stable DCO-style oscillator with additive pulse, saw, sub, and optional noise sources.
- Add a Juno-style non-resonant high-pass stage before or around the VCF according to the selected architecture.
- Implement/calibrate a dedicated four-pole Juno VCF response, including resonance loss and self-oscillation policy.
- Preserve the Juno envelope ranges already documented.
- Decide whether the VCA supports envelope and gate modes; if only envelope mode remains, document the scope explicitly.
- Preserve six-voice-inspired stable tuning while retaining the product's three/four-note chord and two-group overlap design.

### 4. Improve the Juno chorus without destabilizing the voice work

- Retain the current two-mode stereo delay geometry as a baseline.
- Calibrate dry/wet gain so enabling chorus does not create an unintended loudness jump.
- Add optional bandwidth limitation and subtle BBD/compander coloration only if it can remain stable and inexpensive.
- Keep noise optional or omit it deliberately; do not force vintage noise into every patch.
- Preserve stereo spread and release-tail layout through chorus mode changes.

### 5. Implement the SH-101-inspired Lead path

- Use an additive source mixer for saw, pulse, selectable sub, and noise.
- Support the defining one-octave/two-octave sub choices internally; expose them only through a properly designed discrete model control.
- Add keyboard tracking to the filter.
- Implement/calibrate a dedicated SH-101 four-pole VCF response rather than sharing Juno constants.
- Support the documented envelope trigger behavior needed for legato Lead phrases.
- Preserve monophonic priority and release semantics used by the sequencer.

### 6. Separate instrument render functions

- Replace the large `if v.bass`/`if v.chord` render branch with dedicated Bass, Chord/Juno, and Lead/SH render functions or strategy state.
- Keep common mixers and smoothing utilities small and allocation-free.
- Make instrument-specific drive, filter, envelope, and source behavior visible in types rather than boolean flags.

### 7. Calibrate and document

- Establish dry reference patches for pulse, saw, sub, high resonance, envelope sweep, and chorus.
- Level-match source combinations and resonance settings.
- Update `SPEC.md` to distinguish implemented hardware-inspired behavior from omitted hardware controls.
- Update physical readouts and help text for any added controls.

## Tests

- Pulse and saw can contribute simultaneously under the chosen parameter mapping.
- Sub oscillator pitch/mode is correct for each voice.
- Noise is deterministic, finite, and voice-local.
- Juno and SH filter impulse/frequency responses are measurably distinct.
- Keyboard tracking changes SH cutoff by the documented amount.
- Chord overlap retains independent release tails, spread, locks, and chorus state.
- Lead legato and retrigger modes follow the documented gate behavior.
- Chorus remains centered with spread Off and preserves stereo input with spread enabled.
- All oscillator/filter extremes remain finite at supported sample rates.
- Offline render determinism and callback allocation safety continue to pass.

## Status (2026-08-08)

Complete. The implementation uses format version 17; earlier formats are intentionally rejected without migration. It includes persisted optional noise, selectable SH-101 sub modes, 0–100% keyboard tracking, and source-armed Lead portamento. `cargo fmt`, `cargo test`, Clippy with warnings denied, and the release build passed after the changes.

## Completion criteria

- Chord and Lead no longer differ only by drive, resonance scaling, and ADSR ranges.
- Their source mixers and filter types encode the intended instrument identities.
- The intentional format-17 breaking-change policy is documented in `README.md` and `SPEC.md`.
- New controls, if any, follow the full model/reducer/audio/TUI/persistence path.
- Manual checks cover dry, chorus, high-resonance, bass, pad, pluck, and lead patches.
- `cargo fmt`, `cargo test`, `cargo clippy`, and `cargo build --release` pass.
