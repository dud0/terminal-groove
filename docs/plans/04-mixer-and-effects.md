# Implementation Plan: Chord Mixer and Effects Correctness

## Goal

Correct chord-group mixer ownership, prevent effect controls from entering poor operating regions, and improve insert-effect behavior without combining this work with the larger instrument-model redesign.

These items belong together because they affect post-voice stereo mixing, sends, effect-tail ownership, gain staging, and the tests used to validate release tails.

## Findings addressed

- A5: chord release tails use the active group's level and sends.
- A8: flanger modulation flattens at extreme settings.
- Supporting effect-quality and gain-staging observations from the audit.

## Work plan

### 1. Mix chord groups independently

- Render the current chord group and previous releasing group into separate stereo accumulators.
- Apply each group's latched level, pan/spread layout, delay send, and reverb send before combining groups.
- Preserve per-voice panning within each group.
- Feed the combined stereo dry signal through chorus and insert effects according to the intended signal order.
- Define whether chorus and insert effects are shared by both groups or captured per group; prefer shared track effects unless lock semantics require separate state.
- Calculate sends from each correctly gained group so a new lock cannot reroute an old tail.

### 2. Correct preview chord mixing the same way

- Mirror live group ownership in the preview chord path.
- Keep audition state independent from live effect state.
- Ensure a new audition either releases or replaces the prior preview according to the documented policy without borrowing its level/send values.

### 3. Define safe flanger modulation geometry

- Replace raw `center + sin * depth` clamping with a mapping that guarantees the full modulation range stays above the minimum delay.
- Options to evaluate:
  - cap effective depth to `center - minimum_delay`;
  - map depth as a percentage of the available range around center;
  - derive lower/upper delay endpoints from the two controls and interpolate sinusoidally.
- Choose one behavior, update physical readouts, and document how extreme values interact.
- Smooth any derived endpoint changes to avoid read-position discontinuities.

### 4. Review insert-effect gain and feedback topology

- Level-match dry/wet distortion at low and high drive.
- Verify phaser feedback remains stable and intentionally stereo when channels differ.
- Confirm flanger feedback remains independent per channel and finite at 90%.
- Avoid relying on the master limiter to hide excessive internal gain.
- Retain exact dry bypass when wet mix is zero.

### 5. Evaluate reverb and chorus gain staging

- Confirm the Juno chorus mode gains do not create unintended loudness increases.
- Confirm the reverb's wet normalization remains stable across RT60 and tone extremes.
- Do not replace the current Freeverb-style network unless listening tests identify a concrete defect; a more sophisticated reverb is optional future work.

### 6. Update specification and UI readouts

- State explicitly that Chord group level and sends are captured per triggered group through release.
- Describe the chosen flanger center/depth constraint.
- Make the flanger readout show the actual lower and upper delay range, not a nominal range that the DSP later clamps.

## Tests

- A releasing chord with a low level lock is not raised by a new high-level chord.
- A releasing chord keeps its delay and reverb sends after a new chord with different locks starts.
- Active and tail groups preserve their own spread/pan layouts.
- Live and preview chord paths follow the same ownership rules while keeping independent state.
- Every flanger center/depth combination produces a smooth, positive delay trajectory without a clipped plateau.
- Flanger parameter changes crossfade or smooth without clicks.
- Insert effects remain finite under maximum drive, depth, and feedback.
- Zero-mix bypass remains sample-exact.
- Existing delay, reverb, chorus, chord-overlap, lock, and deterministic-render tests pass.

## Completion criteria

- Chord group mixer parameters are applied before groups are combined.
- Old chord tails cannot inherit a later group's level or send locks.
- Flanger UI values describe the delay range actually rendered by the DSP.
- No extreme valid effect setting produces a clamped half-cycle or non-finite state.
- `SPEC.md` and TUI help/readouts match the implemented signal flow.
- `cargo fmt`, `cargo test`, `cargo clippy`, and `cargo build --release` pass.

## Follow-up status (2026-08-09)

Implemented and corrected. Each Chord group now records its valid voice count so a reused group cannot retain an obsolete fourth voice. Insert effects retain their independent Chord-group state and now finish feedback-aware tails before sleeping and clearing state.
