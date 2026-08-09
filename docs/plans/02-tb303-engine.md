# Implementation Plan: TB-303 Bass Engine

## Goal

Replace the generic Bass signal path with a dedicated TB-303-inspired voice whose oscillator, filter, VCA envelope, filter envelope, accent, and slide behavior interact correctly.

These changes belong together because changing only the filter or only the envelope would leave the defining accent/slide/gate interactions incorrect and would make calibration misleading.

## Findings addressed

- A1: Bass VCA and filter incorrectly share one envelope.
- A2: Bass uses a generic four-stage filter instead of a dedicated 303 topology.

## Target behavior

- Monophonic saw or square oscillator with bounded, band-limited output.
- Dedicated nonlinear four-pole, diode-ladder-inspired resonant low-pass model at sufficient oversampling.
- A filter envelope controlled by Env Mod and Decay.
- Separate VCA/gate behavior with fixed hardware-inspired timing.
- Accent that increases loudness and filter excitation through a dedicated contour.
- Slide that produces legato pitch movement and correct envelope retrigger/hold behavior.
- Stable, finite output across supported sample rates and extreme parameters.

## Work plan

### 1. Split Bass state from the generic synth voice

- Introduce a dedicated `BassVoice` or a distinct Bass DSP state embedded in `SynthVoice`.
- Store separate VCA and filter-envelope states.
- Store accent-envelope and accent-history state required for consecutive accents.
- Keep the frequency smoother and explicit slide-armed state.
- Remove Bass-only branches from generic Juno/SH voice processing where practical.

### 2. Implement a dedicated 303 filter

- Add a named 303 filter type in `src/dsp.rs` rather than overloading `LadderFilter`.
- Model four cascaded nonlinear stages with resonance feedback from the final stage.
- Preserve the loop's resonance-dependent passband loss without post-filter makeup, with appropriate internal drive.
- Determine whether 2x oversampling is sufficient through alias/stability tests; use 4x only if measurements justify the callback cost.
- Cache coefficients at control rate and interpolate cutoff to avoid zipper noise.
- Add an explicit reset/finite-recovery path.

### 3. Implement independent envelope behavior

- Add a fast fixed VCA attack and hardware-inspired gate/release response.
- Make held notes remain audible instead of decaying to zero with the filter Decay control.
- Implement a separate exponential filter contour whose duration follows the Bass Decay control.
- Apply Env Mod only to filter-envelope depth.
- Confirm empty steps release the VCA while ties hold it.

### 4. Calibrate accent interaction

- Give accent a dedicated amplitude boost and filter contour.
- Define how consecutive accented steps accumulate or retrigger accent state.
- Ensure ties do not accidentally create a new accent unless the documented articulation requires it.
- Keep accent and slide behavior deterministic under retriggers, probability gating, pause/resume, and project edits.

### 5. Validate slide semantics

- Preserve the existing convention that a slide flag on one step glides into the following note.
- Keep pitch movement legato and avoid VCA/filter-envelope retrigger when sliding.
- Decide whether the fixed 60 ms glide remains the product behavior or should become tempo/gate dependent; document the result in `SPEC.md`.
- Cover cold starts, consecutive slides, slide into an accented note, and slide followed by an empty step.

### 6. Tune oscillator and gain staging

- Calibrate saw and square asymmetry before the filter.
- Set internal drive so resonance and accent generate character without depending on the master limiter.
- Match loudness between saw/square and accented/unaccented notes.
- Remove unexplained post-filter gain constants after calibration.

### 7. Update specification and physical readouts

- Describe the dedicated four-pole model, its gentle cutoff transition, and its approximately 24 dB/octave far-stopband behavior.
- Document separate VCA, filter, and accent contours.
- Keep cutoff, resonance, Env Mod, and Decay controls stable unless a model/schema change is justified.
- Update any readout whose stated physical range changes during calibration.

## Tests

- A long held/tied Bass note remains audible after the filter envelope reaches its floor.
- An empty step releases the VCA with the documented timing.
- Filter-envelope Decay changes timbre without directly changing the VCA decay.
- Resonance remains finite and stable at minimum/maximum cutoff and all supported sample rates.
- The filter approaches 24 dB/octave in a controlled far-stopband response test.
- Accent increases both peak level and filter brightness without exceeding the expected internal range.
- Consecutive accents, retriggers, ties, and slides are deterministic.
- Slide reaches the documented target in the documented time and does not retrigger the wrong envelopes.
- Offline rendering remains deterministic and callback allocation-free.

## Completion criteria

- Bass no longer uses the generic ADSR as both VCA and filter envelope.
- Bass no longer uses the generic four-stage `LadderFilter`.
- Accent, slide, tie, and gate behavior have focused regression tests.
- Sound calibration is manually checked through dry and moderately distorted acid patterns.
- `SPEC.md` describes the implemented behavior accurately.
- `cargo fmt`, `cargo test`, `cargo clippy`, and `cargo build --release` pass.

## Follow-up status (2026-08-09)

Implemented and corrected. The dedicated VCA, filter contour, and accent contour remain independent; the idle fast path now clears residual filter/accent state so a later unaccented note cannot inherit frozen accent energy.

The intermediate three-stage filter was subsequently replaced by a four-stage nonlinear ladder. Resonance-dependent post-filter makeup was also removed from Bass, Chord, and Lead so their feedback loops determine the natural passband loss and cutoff emphasis.
