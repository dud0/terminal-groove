# Implementation Plan: Real-Time Performance and Callback Safety

## Goal

Reduce worst-case callback load and establish automated proof that the callback does not allocate, block, or regress beyond an agreed performance budget.

This work should be completed as one project because preview gating, coefficient caching, control-rate updates, and performance tests affect the same hot path and need to be measured together.

## Findings addressed

- A3: idle preview processing duplicates callback work.
- A4: expensive mappings and coefficients are recalculated per sample.
- A9: callback allocation and worst-case load lack automated verification.

## Design constraints

- Preserve sample-accurate scheduling, envelopes, pitch glide, and LFO phase behavior.
- Do not allocate, deallocate, lock, log, format, or access files in the callback.
- Keep all buffers and state preallocated when the stream/renderer is built.
- Preserve deterministic offline rendering.
- Preserve smoothing across live project replacements and parameter locks.
- Do not optimize by lowering synthesis quality without an explicit measured tradeoff.

## Work plan

### 1. Establish repeatable performance fixtures

- Add an offline worst-case project builder under `#[cfg(test)]` or a dedicated benchmark target.
- Exercise all drum tracks, Bass, four-voice Chord overlap, Lead, active sends, long reverb, and every insert-effect type.
- Include representative LFO assignments rather than impossible assignments rejected by the model.
- Measure release-mode nanoseconds per rendered frame and callback load for 128-, 256-, and 512-frame buffers at 44.1, 48, and 96 kHz.
- Record the machine/compiler context with benchmark output; avoid a brittle absolute timing assertion in ordinary unit tests.

### 2. Add callback allocation instrumentation

- Add a test-only counting allocator or equivalent scoped allocation detector.
- Construct the renderer, queues, projects, and output buffers before entering the measured region.
- Cover command draining, rendering, project replacement, retirement-queue saturation, Stop, audition, and effect clearing.
- Assert zero allocations and deallocations during the callback invocation.
- Explicitly test the pending-snapshot path when the retirement queue is full.

### 3. Gate preview work by activity

- Add explicit preview activity state that accounts for:
  - non-idle preview drum and synth envelopes;
  - active preview chord voices or arpeggio;
  - scheduled preview retriggers;
  - chorus and insert-effect tails.
- Advance only the auditioned track's preview LFO bank while its preview path is active.
- Skip preview voice mixing and panning for completely inactive tracks.
- Let preview insert effects finish their actual feedback tails before sleeping.
- Reset preview state deterministically on Stop and on the next explicit audition as required by the specification.

### 4. Give insert-effect chains activity gates and per-stage bypass

- Track input activity and feedback-tail activity separately for phaser and flanger.
- Bypass distortion completely when its mix is settled at zero, even if a later effect is active.
- Bypass phaser completely when its mix is settled at zero.
- Bypass flanger completely when its mix is settled at zero.
- When input is silent, continue only stateful wet stages until their feedback state falls below the silence threshold or a conservative bounded tail expires.
- Ensure parameter ramps still advance while necessary; settle them without running unrelated audio math.

### 5. Move invariant calculations to configuration/control rate

- Extend smoother/control structures to cache physical values and coefficients.
- Cache ADSR target times and one-pole coefficients when a parameter or LFO control value changes.
- Compute sidechain attack/release coefficients in `configure`; compute one ducking gain per output frame and reuse it for Bass, Chord, and Lead.
- Cache static pan gains and update them only while pan smoothing or a pan LFO is active.
- Cache distortion drive, tone coefficient, phaser rate/sweep constants, and flanger rate/delay scaling.
- Calculate each synth filter coefficient once per oversampled frame, not once per oversampling iteration.
- Where LFO modulation requires continuous updates, use a bounded control rate with interpolation and document the maximum modulation error.

### 6. Avoid full LFO-matrix scans

- Precompute a fixed-size enabled-destination list or bitset per track when applying a project snapshot.
- Advance only enabled LFOs.
- Preserve disabled-destination zeros without rewriting all 33 offsets every sample.
- Maintain independent live and preview phases and existing reset/freeze semantics.

### 7. Re-measure and document the callback budget

- Compare baseline and optimized worst-case render cost.
- Test with the null ALSA device at the documented 512-frame automatic buffer and smaller explicit buffers where supported.
- Confirm the overrun counter remains zero during a sustained worst-case manual run on the reference development machine.
- Document expected headroom rather than claiming universal dropout-free performance.

## Tests

- Callback invocation performs zero allocations and deallocations.
- Idle renderer output remains deterministic while preview state performs no DSP advancement.
- Preview LFO phase begins at zero on audition and matches the current behavior while active.
- Effect tails complete rather than being cut off by activity gates.
- Cached/control-rate output remains finite and within a defined tolerance of the prior implementation for static parameters.
- Pan, cutoff, resonance, and effect automation remain smooth.
- Existing timing, LFO, audition, effect, and offline-render tests continue to pass.

## Completion criteria

- Allocation-safety tests cover normal and queue-pressure callback paths.
- Idle preview cost is near zero apart from a fixed activity check.
- Disabled effect stages do not execute their DSP.
- No invariant exponential/trigonometric mapping remains in an inner oversampling loop.
- Worst-case release benchmarks show a material improvement and are documented.
- `cargo fmt`, `cargo test`, `cargo clippy`, and `cargo build --release` pass.
