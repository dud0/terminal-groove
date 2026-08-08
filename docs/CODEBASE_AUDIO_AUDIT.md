# Codebase and Audio Audit

Date: 2026-08-08

## Scope

This audit covered the model, reducer, persistence, sequencing, real-time audio callback, synthesis voices, mixer, effects, TUI-to-audio synchronization, and existing tests. The audio review emphasized callback reliability and the intended TR-909, TB-303, Juno-60, and SH-101 influences.

The repository was audited without changing runtime behavior. At the time of the audit:

- `cargo test` passed all 241 tests.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo fmt --all -- --check` passed.
- `cargo build --release` passed.

## Overall assessment

The codebase has a strong foundation. The callback owns preallocated voice and effect state, UI-to-audio communication uses bounded SPSC queues, project snapshots are retired outside the callback, and the model and persistence layers have broad boundary and rejection-path coverage.

The primary risks are concentrated in the DSP implementation:

1. The TB-303 voice uses an incorrect shared amplitude/filter envelope and a four-stage filter.
2. The callback performs substantial unnecessary preview work and many per-sample transcendental calculations.
3. Active and releasing chord groups are combined before their separately latched mixer values are applied.
4. The Juno-60 and SH-101 voices share too much generic synthesis code to reproduce their distinct source mixers and behavior convincingly.
5. Several effects are competent utility implementations but have correctness or quality limitations at extreme settings.

No additional high-confidence correctness defect was found in model validation, reducer history, persistence, or the TUI control path during this audit.

## Findings

### A1. High: the Bass voice shares one envelope between its VCA and filter

`Renderer::render_synth` obtains one ADSR sample, uses it to move filter cutoff, and multiplies the final audio sample by the same value. Bass configuration fixes sustain at zero and maps the Bass Decay control into that ADSR.

Consequences:

- A held or tied Bass note eventually fades to silence according to the filter Decay control.
- Filter articulation and note loudness cannot follow separate TB-303-style behavior.
- Accent filter movement is coupled to the same generic envelope instead of having a dedicated contour.
- Slide can preserve legato pitch behavior while the shared envelope still fades the voice away.

Relevant code:

- `src/audio/synthesis.rs`, `apply_synth_params_core`
- `src/audio/renderer.rs`, `render_synth`
- `src/audio/voices.rs`, `SynthVoice`

Resolution: [TB-303 implementation plan](plans/02-tb303-engine.md).

### A2. High: the Bass filter is a generic four-stage ladder

The Bass voice uses the same `LadderFilter` structure as the other Roland-inspired voices. It is a nonlinear four-stage low-pass with feedback approaching 3.85. A TB-303-inspired implementation should use a dedicated three-pole/18 dB topology and calibrate resonance, cutoff loss, drive, and oversampling for that model.

The current filter can sound resonant and acidic, but its slope, phase response, feedback behavior, and resonance coloration differ materially from the target.

Relevant code:

- `src/dsp.rs`, `LadderFilter`
- `src/audio/renderer.rs`, `render_synth`

Resolution: [TB-303 implementation plan](plans/02-tb303-engine.md).

### A3. High: idle preview processing duplicates callback work

Preview LFO state is advanced for all 33 parameter IDs on all eight tracks every sample, whether or not an audition is active. Preview insert effects are also called for every track. When an effect has a nonzero wet mix, `TrackEffectChain` remains processing even for an indefinitely silent input, so an unused preview chain can duplicate distortion, phaser, and flanger work.

This becomes especially expensive when several tracks have enabled phaser or flanger settings. It is avoidable because preview state only needs sample-accurate advancement while an audition voice, scheduled preview retrigger, or preview effect tail is active.

Relevant code:

- `src/audio/renderer.rs`, `next`
- `src/audio/scheduler.rs`, `advance_preview_lfos`
- `src/dsp.rs`, `TrackEffectChain::process_stereo`

Resolution: [real-time performance plan](plans/01-real-time-performance.md).

### A4. High: expensive coefficients and mappings are recalculated per sample

The hot path repeatedly evaluates `powf`, `exp`, `sin`, `cos`, `tan`, and multiple `tanh` calls. Examples include:

- ADSR percentage-to-time conversion and coefficients for every active voice.
- Cutoff mapping and filter-envelope exponentiation for every active voice.
- Ladder coefficients inside both oversampling iterations.
- Equal-power panning for idle and active live/preview paths.
- Sidechain gain conversion multiple times per output frame.
- Distortion, phaser, and flanger parameter mappings and coefficients.
- LFO smoothing coefficients.

Some nonlinear operations are inherent to the synthesis model, but parameter mappings and invariant coefficients should be computed when values change or at a bounded control rate. The current implementation increases dropout risk at small callback buffers and worst-case polyphony/effect settings.

Relevant code:

- `src/audio/renderer.rs`, `render_synth` and `next`
- `src/dsp.rs`, `Adsr`, `LadderFilter`, `SidechainCompressor`, and `TrackEffectChain`

Resolution: [real-time performance plan](plans/01-real-time-performance.md).

### A5. Medium: chord tails use the active group's level and sends

All chord voices, including the current group and the previous releasing group, are summed before track level and sends are selected. If a new chord is active, its first voice supplies the level, delay send, and reverb send applied to the complete aggregate.

When consecutive chords have different parameter locks, the old tail is therefore rescaled and rerouted using the new chord's values. If no active group exists, the first discovered tail supplies those values for all remaining tails.

Relevant code:

- `src/audio/renderer.rs`, the live and preview chord mixing blocks
- `src/audio/synthesis.rs`, alternating chord voice groups

Resolution: [mixer and effects correctness plan](plans/04-mixer-and-effects.md).

### A6. Accepted design: hats and cymbals are intentionally sample-free

The original TR-909 used sampled sources for its hats and cymbals, while this project uses band-limited square oscillators, deterministic noise, filtering, and nonlinear coloration. This is an intentional product choice and is not considered a defect.

`SPEC.md` now describes both voices as sample-free metallic percussion that complement the 909-inspired analog drum voices without claiming to emulate the original sampled sources. No synthesis change is planned.

The kick is the strongest 909-like implementation. The snare and tom are effective procedural approximations, though they are not detailed circuit models. Their current architecture is suitable unless closer circuit-level authenticity becomes a separate goal.

### A7. Medium: Juno-60 and SH-101 behavior is insufficiently differentiated

Chord and Lead use the same band-limited oscillator structure and the same generic four-stage filter. Their primary differences are envelope ranges, drive, and a resonance scaling constant.

Important target characteristics not represented directly include:

- Juno-60 independently additive pulse and saw sources rather than an exclusive crossfade.
- Juno-60 noise and high-pass stages.
- Juno-60-specific VCF/VCA calibration and optional gate-style VCA behavior.
- SH-101 independently mixed pulse, saw, sub, and noise sources.
- SH-101 selectable sub-oscillator octaves/wave shapes.
- SH-101 keyboard tracking and characteristic gate/trigger behavior.
- Instrument-specific filter and resonance calibration.

The current Juno chorus is a useful approximation and should be retained as a starting point, but it is a clean modulated-delay model rather than a detailed BBD/compander implementation.

Relevant code:

- `src/audio/renderer.rs`, `render_synth`
- `src/audio/synthesis.rs`, `apply_synth_params_core`
- `src/audio/voices.rs`, `SynthVoice`
- `src/dsp.rs`, `PolyBlepOsc`, `LadderFilter`, and `StereoChorus`

Resolution: [Juno-60 and SH-101 plan](plans/03-juno-sh101-voices.md).

### A8. Medium: flanger modulation flattens at extreme settings

The flanger exposes a center delay down to 0.2 ms and a modulation depth up to 5 ms. Delay values below 0.1 ms are clamped. At low-center/high-depth settings, most of the negative half-cycle is pinned to the minimum delay instead of following a sinusoid, creating an asymmetric sweep and a long stationary region.

The parameter domain should prevent invalid center/depth combinations, or the DSP mapping should convert the user controls into a modulation range that always remains positive and smooth.

Relevant code:

- `src/dsp.rs`, `TrackEffectChain::flanger_delay_samples`
- `src/model.rs`, `FlangerParameters`
- `src/tui/render.rs`, flanger physical readouts

Resolution: [mixer and effects correctness plan](plans/04-mixer-and-effects.md).

### A9. Test gap: allocation safety and worst-case callback load are not verified

The normal callback path appears intentionally allocation-free, but the acceptance specification calls for a callback-path allocation test and none is present. There is also no repeatable worst-case performance fixture covering maximum chord overlap, all tracks, active LFOs, insert effects, global sends, project replacement, and small callback buffers.

Callback overrun telemetry is useful in production, but it cannot replace regression tests and reproducible performance measurements.

Resolution: [real-time performance plan](plans/01-real-time-performance.md).

## Effects quality summary

### Delay

The delay is a strong utility implementation. It is preallocated, crossfades time changes, filters feedback, supports stereo ping-pong behavior, and sleeps after its estimated tail.

### Reverb

The reverb is a correct basic Schroeder/Freeverb-style design with stereo pre-delay, input high-pass filtering, damping, and RT60-based comb feedback. Its likely limitation is a static or metallic tail compared with a modulated feedback-delay network, not an immediate correctness defect.

### Chorus

The chord chorus provides deterministic stereo modulation, two modes, transition crossfades, and activity gating. It is musically useful but does not model BBD bandwidth, clock feedthrough, companding, or noise. Such coloration should only be added after the core voice and callback work, and should remain optional.

### Insert effects

Distortion, phaser, and flanger are functional utility effects. They are not circuit models. The most important work is activity gating, coefficient caching, per-stage bypass, and correcting the flanger parameter domain before adding further sonic complexity.

## Implementation grouping

The work is split by shared architecture and verification needs:

1. [Real-time performance and callback safety](plans/01-real-time-performance.md)
2. [TB-303 Bass engine](plans/02-tb303-engine.md)
3. [Juno-60 Chord and SH-101 Lead voices](plans/03-juno-sh101-voices.md)
4. [Chord mixer and effects correctness](plans/04-mixer-and-effects.md)

The plans are ordered by risk reduction. The real-time plan should land first so subsequent synthesis work can use cached/control-rate DSP infrastructure and can be measured against an established callback budget.
