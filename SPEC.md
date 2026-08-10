# terminal-groove MVP specification

Status: implementation-ready MVP specification  
Target: Linux-first terminal application  
Application and executable name: `terminal-groove`

## 1. Product definition

`terminal-groove` is a real-time terminal groovebox with a fast keyboard workflow, predictable state transitions, and visible selection, transport, editing state, parameters, events, locks, shortcuts, dirty state, and audio errors. It has nine independently looping 1–64-step tracks on a shared sixteenth-note clock; all sound is synthesized and tracks default to 16 steps.

The fixed track order is:

1. Kick drum
2. Snare drum
3. Hi-hat
4. Tom drum
5. Cymbal drum
6. Rimshot
7. Bass
8. Chord
9. Lead

### 1.1 MVP capabilities

Playback supports start, pause, resume, stop, reset, live editing, drum triggers, synth notes and ties, per-step locks, per-track mixing and sends, eligible parameter LFOs, tempo/effects/key/scale configuration, non-destructive audition, up to 100 patterns, session undo/redo, versioned human-readable JSON, and default or explicitly selected audio output.

### 1.2 Explicitly excluded from the MVP

- Samples, sample import, or sample playback
- MIDI input, output, or clock sync
- WAV or other audio export
- Song-mode playback or song-editing UI. The project retains a persisted song reference list for forward-compatible project data, but the MVP does not play or edit that list.
- Time-signature changes
- Microtiming or continuously variable event velocity
- Polyphonic note entry outside the fixed Chord shape mapping, or oscillator detune
- Solo, master-volume control, or configurable effect returns
- User-defined track types or track reordering
- Mouse control
- Plug-ins or external effects

## 2. Sequencer model

### 2.1 Timing and transport

- Every track has 1 through 64 steps, with one step equal to one sixteenth note. Tracks advance together and wrap independently at their configured lengths.
- At tempo `BPM`, the nominal duration of one step is `sample_rate * 60 / (BPM * 4)` samples.
- The audio engine must use fractional sample accumulation rather than rounding every step independently, preventing cumulative timing drift.
- Starting from the reset state triggers step 1 of every track immediately.
- `Space` while playing pauses before the next unplayed step. Active synth gates are released, while delay and reverb tails continue.
- `Space` while paused resumes by triggering the next unplayed step immediately and establishing a new timing origin.
- `.` stops playback, resets the active pattern and every track's next step to step 1, releases all voices, and clears delay and reverb state.
- The edit cursor is independent from the playhead.
- Tempo changes become active at the next step boundary without skipping or repeating a step.
- Changing a track length while playing preserves its next local step when that step remains in range; otherwise that track wraps to step 1. Other tracks do not restart.
- Growing a track appends empty steps. Shrinking truncates removed steps immediately and clears ties made invalid by the new cyclic boundary. The complete resize and tie cleanup are one undoable edit.
- Doubling a track of 1 through 32 steps appends an exact copy of all existing steps, including events and locks. Tracks longer than 32 steps cannot be doubled because the result would exceed 64.

### 2.2 Patterns

- A project contains 1 through 100 dynamic patterns, numbered from 1. Each pattern owns the nine track sequences; instrument, mixer, global, and effect settings are shared. New projects contain one empty pattern.
- The MVP plays patterns directly and does not play the persisted `song` reference list. Pattern edits keep that list's one-based references valid where possible.
- `Ctrl+P` opens a horizontally organized pattern dialog. Left/right, `Home`, and `End` move a visual cursor without changing playback. `Enter` selects the cursor pattern while stopped or queues it for the next bar while playing, then closes the dialog. `N` inserts an empty pattern after the cursor, `D` duplicates it, `C` copies it, `X` cuts it, `V` pastes the copied pattern after the cursor, and `Delete` removes it. The final pattern cannot be removed and is reset to empty.
- The dialog marks the currently playing pattern with `▶`, the next queued pattern with `⏭`, and empty patterns with a muted style. The pattern strip scrolls horizontally when necessary.
- Pattern insertion, deletion, and replacement rebase active and queued playback indexes so queued playback continues to refer to the same pattern where possible.

### 2.3 Drum events

A drum step is either empty or contains one trigger with a required Boolean `accent`, a trigger `condition`, and a `retrigger_count`. Hi-hat and Tom triggers additionally select a referenced instrument recipe: Hi-hat recipes are Closed and Open, while Tom recipes are Low, Medium, and High. Recipe 1 is the default and is omitted from JSON. Each track has a persisted input accent default, initially off. New triggers inherit that default; pressing `A` on an empty step toggles the default and leaves the step empty. Pressing `A` on an occupied trigger toggles only that trigger's accent. New triggers otherwise always trigger and retrigger once. `condition` is `Always`, `Cycle { position, length }` for all phases of lengths 2–4, or `Chance { probability }` at 0–100%. Counts are 1–4 inclusive, including the first hit. Ties never carry these fields. Pressing `Enter` on an occupied drum step clears the trigger, its articulation, and all locks.

On the Hi-hat row, `1` and `2` select Closed and Open. On the Tom row, `1`, `2`, and `3` select Low, Medium, and High. The shortcut creates a trigger on an empty step or changes the recipe on an existing trigger, preserving accent, condition, retrigger count, and unrelated locks. Selecting a recipe clears that step's Tune/Tone/Decay overrides as applicable. `0` clears those overrides without changing the recipe. Recipe selection automatically auditions while stopped or paused.

Each drum track is a single retriggerable synthesized voice. Accent has a fixed engine-specific response: it raises level and may also strengthen transient excitation or brightness, including the Rimshot's high-frequency crack. Instrument parameters are captured when the hit starts and remain part of that hit's tail. Mixer level and sends continue to follow each sequencer step.

### 2.4 Synth events

A synth step is exactly one of:

- Empty
- A note containing a scale degree from 1 through 8, an input octave from 0 through 7, and a Boolean accent
- A tie

Degree 1 is the root in the stored input octave. Degrees 2 through 7 use the selected scale, and degree 8 is the root one octave above degree 1. Pitch uses twelve-tone equal temperament with A4 = 440 Hz.

Notes are stored as scale degree and octave rather than absolute pitch. Changing the global key or scale therefore reinterprets all existing pattern notes the next time they trigger. A note that is already sounding keeps its current pitch until a new note event starts; a tie does not retune it.

Every note also owns the same `condition` and `retrigger_count` fields as a drum trigger. A skipped Bass, Chord, or Lead note acts as an empty step: it releases the active voice and applies no locks. A skipped drum trigger is silent.

The shared track probability gate applies to both drum triggers and pitched notes. At a reached step, the engine evaluates the event condition first, then evaluates the track probability only for an eligible trigger or note whose condition passed. A successful gate schedules the base action and its complete retrigger burst. A rejected drum trigger is silent; a rejected Bass, Chord, or Lead note is processed exactly like an empty step and releases the active voice without applying locks. Ties bypass probability and retain the existing hold/release behavior.

Bass and Lead are monophonic. Chord interprets the stored degree as the root of a selected diatonic shape. The available shapes, in selector order, are the single-note `1`, dyads `1-3` and `1-5`, then `1-3-5`, `1-3-5-7`, `1-3-5-6`, `1-2-5`, and `1-4-5`, plus the existing three- and four-note shapes' cyclic inversions. A shape is stored on each Chord note as an ordered scale-degree recipe, so its musical quality follows the current root and scale. The `1` shape plays only the degree and octave stored on the step. For example, `1-3-5-7` can produce different seventh qualities depending on the root and scale.

- A Bass or Lead note sets pitch, captures its accent, opens the gate, and retriggers its amplitude envelope.
- A Chord note renders its selected shape in close position. The stored octave is the root octave; when an inversion recipe wraps from a higher degree to a lower degree, the wrapped tone rises by one scale octave.
- Chord has two alternating four-voice groups. A new note releases the previous shape and retriggers all of its tones, including common tones; one released shape may overlap before the oldest group is reused.
- A following tie keeps the mono voice or complete chord shape open without retriggering pitch, accent, or envelopes. A tie inherits the source note's Chord shape and arpeggio configuration and cannot override or restart them.
- A new Chord note restarts its own arpeggio sequence. An empty step releases the arpeggio; the next note starts a new cycle.
- A following empty step closes the gate and begins release.
- A following note closes/restarts the existing voice at the new pitch, with click-safe envelope handling.
- Bass and Lead notes additionally store a Boolean slide. Bass slides remain armed through ties and glide to the next Bass note over a fixed 60 ms. Lead slide is SH-101 AUTO portamento: it uses the source note's effective Portamento Time, glides in pitch without retriggering ADSR, remains armed through ties, and is cleared by an empty step.

Pressing a degree key replaces any existing event on the selected step with that note and preserves compatible locks, articulations, and the Chord shape/arpeggio of an existing Chord note. New notes inherit the track's input accent default and new Bass and Lead notes have slide disabled; replacing a tie or creating a note on an empty step uses the current default, while replacing an existing compatible note preserves its accent. Pressing `Enter` on an empty pitched step inserts the track's last-entered degree and octave with the input accent default; empty Chord steps also use the track's last-entered Chord shape and arpeggio configuration. Pressing `Enter` on a note or tie clears it and its locks.

### 2.5 Tie invariants

- A tie is valid only when its immediately preceding step, considered cyclically, is a note or another valid tie that resolves to a note.
- Step 1 may tie cyclically from the track's final step.
- A track sequence containing only ties is invalid.
- Adding a tie to an empty step requires a valid predecessor. Otherwise, the edit is rejected and a visible status message explains why.
- Pressing `t` on a tie clears it and its locks.
- Pressing `t` on a note replaces the note with a tie only if the resulting tie graph remains valid.
- Clearing or replacing a source note also clears the contiguous following ties that would become invalid, including a wrapped chain. This is recorded as one undoable operation.
- When playback or audition begins on a tie without an already-active voice, the engine resolves the source note cyclically and retriggers it at that boundary to establish the held voice. Continuous playback across the loop does not retrigger a valid wrapped tie.
- Ties do not store accent. They inherit the accent captured from their resolved source note, including across a wrapped tie chain.

### 2.6 Parameter locks

Every track has base parameter values. A step may contain a sparse set of parameter locks that overlay those values for that step only.

- Locks are permitted only on a drum trigger, synth note, or synth tie.
- Instrument parameters, waveform, pan, level, delay send, reverb send, distortion, and phaser parameters are lockable. Chord spread is also lockable on Chord steps. Effects are not LFO destinations.
- Mute is never lockable.
- At each boundary, the engine resolves the referenced Hi-hat or Tom recipe, then overlays the current step's locks.
- A track LFO then applies its bipolar offset around that effective base-or-lock value and clamps the result to 0–100%.
- At the next boundary, an unlocked parameter returns to its base value or takes the next step's lock.
- Continuous changes are smoothed to prevent clicks.
- A drum instrument lock initializes the triggered voice and therefore remains audible for that hit's tail. Step-level mixer locks still expire at the next boundary.
- Pitched-track locks on a tie can update compatible oscillator, filter, envelope, level, send, and Chord chorus settings without retriggering. Effective values flow from the source note through each connected tie; each tie's locks override prior values and remain effective on following ties until overridden. Changing attack during a tie does not restart the attack phase.
- Clearing an event also clears every lock on that step.
- Accent and slide are event properties, not base parameters, locks, or LFO destinations. They apply on the event's next trigger.

The UI has a persistent parameter scope with two visibly labelled states: `BASE` and `LOCK`. `p` toggles the scope on a track. The scope persists while moving between steps and tracks, and resets to `BASE` when the user selects the global row or presses `Esc` from navigation mode.

In `LOCK` scope, attempting to edit a parameter on an empty step is rejected. When a lock does not yet exist, arrow editing begins from the inherited base value. `Backspace` or `Delete` while editing a locked parameter removes only that lock and exits parameter editing.

## 3. Instruments and signal flow

### 3.1 Common value model

Track parameters use an integer `Percent` value from 0 through 100 inclusive. Persistence and UI presentation use the same integer.

- `` ` `` enters 0%.
- `1` through `9` enter 10% through 90%.
- `0` enters 100%.
- Up/down changes the value by 1 percentage point.
- Shift+up/down changes it by 10 percentage points.
- Values clamp at their limits.

Physical controls that need perceptual resolution, such as frequency and envelope time, map the percentage exponentially. The detail panel displays both the percentage and the derived physical value when applicable.

Track level maps 0% to silence and 100% to unity gain using a smooth perceptual curve. Pan is 0–100%, with 50% center, and uses equal-power gains from hard left to hard right. Effect sends map 0% to no send and 100% to the full post-fader, panned track signal. Tracks default to center.

All continuous DSP parameters use short smoothing ramps. A default ramp of approximately 5 ms is sufficient except for delay-time changes, which require a longer click-free crossfade.

### 3.2 Per-parameter LFOs

Each track may attach one independent LFO to each eligible continuous instrument parameter, track level, and pan. Chord and Lead additionally support the LFO-only `pitch` destination. Waveform, mute, delay send, reverb send, accent, slide, Chord spread, Chord/Lead Noise, Lead Keyboard Tracking, and global parameters are not eligible.

Each assignment stores enabled state, waveform, trigger-reset state, starting phase, rate, and depth. Starting phase is a 0–100% cycle position; 0% and 100% are equivalent. Waveforms are sine, triangle, square, rising saw, and deterministic sample-and-hold. At 0%, sine and triangle begin at the center and rise, square begins high, saw begins at -1 and rises, and sample-and-hold selects one deterministic pseudorandom bipolar value per cycle.

Depth is 0–100 percentage points and is bipolar. For ordinary destinations, the engine adds `waveform * depth` to the current effective base-or-lock percentage, clamps to 0–100, and only then maps the percentage to its physical value. `pitch` is LFO-only: it has no base value or step lock, and converts `waveform * depth` to `offset_percent / 100 * 2` semitones around every triggered Chord or Lead note. Thus 100% depth is bipolar ±2 semitones. The frequency multiplier is `2^(semitones/12)`. Discontinuous waveforms receive an approximately 5 ms smoothing stage.

Free rate uses an exponential 0–100 control mapping from 0.01 Hz through 20 Hz. Tempo-synchronized cycle lengths, from slowest to fastest, are four bars, two bars, one bar, half, dotted quarter, quarter, quarter triplet, dotted eighth, eighth, eighth triplet, sixteenth, sixteenth triplet, and thirty-second.

Sequence LFO phase starts at its configured starting phase after explicit Stop or when newly enabled, freezes on pause, and continues on resume. When trigger reset is enabled, each accepted drum or note hit resets the track parameter's LFO to its starting phase before the voice samples modulation. This includes every hit in a configured retrigger burst and a tie that must recover an inactive source voice, but excludes rejected conditions/probability gates, ordinary held ties, empty boundaries, and Chord arpeggiator substeps. The starting waveform value applies exactly on the hit; subsequent discontinuities retain the normal smoothing stage. Changing starting phase while an LFO is running takes effect at its next initialization or qualifying trigger.

Auditions use independent preview LFO state beginning at the configured starting phase. Every new audition initializes that state, and subsequent hits in its retrigger burst reset only assignments whose trigger reset is enabled. Sample-and-hold initialization and trigger resets reproduce the assignment's deterministic seeded value. Drum instrument parameters sample their modulated values when a hit starts; level and pitched-voice destinations modulate continuously, including ties and release tails.

### 3.3 Kick drum

The 909-inspired kick combines a resonant sine body, an exponential pitch transient, a short procedural-noise click, and mild nonlinear coloration.

- `tune`: maps the settled body from 45–70 Hz and pitch peak from 110–280 Hz.
- `decay`: maps exponentially from approximately 80 ms to 1.2 seconds.
- `attack`: changes click strength and pitch-sweep intensity.

Defaults: tune 50%, decay 35%, attack 35%. Accent adds about 4 dB and 25% more transient excitation.

### 3.4 Snare drum

The 909-inspired snare combines two detuned triangle resonators with separately high-pass/band-pass-filtered procedural noise.

- `tune`: moves the lower body from approximately 150–300 Hz and the upper mode at 1.72 times that frequency.
- `tone`: changes resonator balance and noise-band brightness.
- `snappy`: changes noise excitation and its tail from approximately 80–420 ms.

Defaults: tune 50%, tone 50%, snappy 55%. Accent adds about 4 dB and 20% more snappy excitation.

### 3.5 Hi-hat

The hi-hat is an intentionally sample-free metallic percussion voice rather than an emulation of the TR-909's sampled hi-hat source. Six band-limited square oscillators at fixed inharmonic ratios mix with deterministic noise, then pass through high-pass/band-pass shaping and coarse nonlinear coloration. Its tuning, short-to-long decay range, and accent response are designed to sit alongside the 909-inspired analog drum voices while retaining a distinct synthesized character.

- `tune`: moves the metallic source base from approximately 310–670 Hz and its filter bands.
- `decay`: maps exponentially from approximately 25–800 ms, spanning closed to open behavior.

Closed is recipe 1 and defaults to tune 50%, decay 15%. Open defaults to tune 50%, decay 85%. Recipe changes affect all referencing steps on their next hit. Accent adds about 3 dB and a short brightness boost.

### 3.6 Tom drum

The Tom is a 909-style synthesized tom combining two damped triangle resonators, a short deterministic attack click, and mild nonlinear coloration.

- `tune`: maps the fundamental from approximately 80–220 Hz and the upper resonator from 118–326 Hz.
- `tone`: balances the low body, upper resonator, and attack click.
- `decay`: maps exponentially from approximately 90–800 ms.

Low is recipe 1 and defaults to tune/tone/decay 15%/35%/60%. Medium defaults to 50%/50%/45%, and High to 85%/65%/35%. Recipe changes affect all referencing steps on their next hit. Accent adds about 3 dB and a stronger attack click.

### 3.7 Cymbal

The Cymbal is an intentionally sample-free metallic percussion voice rather than an emulation of the TR-909's sampled cymbal source. It is built from six fixed inharmonic square oscillators blended with deterministic noise and shaped by high-pass and band-pass filters. It complements the 909-inspired analog drum voices but deliberately retains its own synthesized character.

- `tune`: maps the metallic source base from approximately 240–720 Hz.
- `tone`: balances the metallic oscillator bank against the filtered noise component.
- `decay`: maps exponentially from approximately 80–1800 ms.

Defaults: tune 50%, tone 55%, decay 30%. Accent adds about 3 dB and a short high-frequency emphasis.

### 3.8 Rimshot

The 909-inspired Rimshot uses three independently damped sine resonators, a short amplitude attack, high-pass shaping, and mild nonlinear coloration. At the default settings, its modes are approximately 222 Hz at half amplitude with a 45 ms nominal -80 dB decay, 500 Hz at full amplitude with a 20 ms decay, and 1 kHz at full amplitude with a 5 ms decay.

- `tune`: exponentially scales all three resonator frequencies from 0.5× through 2×, with 50% at the reference frequencies.
- `tone`: shifts normalized energy from the 222/500 Hz body toward the 1 kHz crack while retaining all three modes.
- `decay`: exponentially scales every modal tail from 0.25× through 4×, with 50% at the reference decay times.

Defaults: tune 50%, tone 50%, decay 50%. Accent adds about 3 dB and 20% more 1 kHz crack excitation.

### 3.9 Bass, Chord, and Lead

The Bass track is a 303-inspired engine with:

- Band-limited asymmetric saw or square waveform.
- A nonlinear four-pole, diode-ladder-inspired resonant low-pass filter processed at 2x oversampling. Its transition around cutoff is deliberately gentle, while its far-stopband response approaches 24 dB/octave.
- `cutoff` mapped exponentially from 80 Hz to 8 kHz, `resonance`, positive `filter envelope` up to five octaves, and an exponential filter-contour `decay` mapped from about 80 ms to 2 seconds.
- A separate fixed fast VCA gate with approximately 3 ms attack and 55 ms release. Held notes remain audible after the filter contour reaches its floor; empty steps release this gate and ties hold it.
- Accent adds about 3 dB through a dedicated short amplitude contour plus extra filter excitation. Each accented note retriggers that contour; ties do not. A Bass slide retains the VCA and filter contours while gliding to its target over the fixed 60 ms.

Defaults: saw, cutoff 45%, resonance 55%, filter envelope 65%, decay 40%.

Chord is a Juno-60-inspired polyphonic engine; Lead is an SH-101-inspired monophonic engine. Both provide phase-aligned band-limited Pulse and Saw sources, an additive source mixer, a nonlinear four-stage resonant low-pass filter, one ADSR for amplitude and positive filter modulation, and these controls:

The Bass, Chord, and Lead filters apply resonance within their feedback loops without resonance-dependent output makeup. Increasing resonance therefore emphasizes the cutoff region relative to the low-frequency passband and may reduce the voice's overall level.

- `oscillator mix`: 0% is Pulse, 100% is Saw. Intermediate values are additive pulse/saw source levels with equal-power macro gains, so both sources contribute simultaneously without changing the saved-project meaning of this control.
- `pulse width`: maps from 5% through 95% duty cycle.
- `sub oscillator`: linear source level; Chord uses an octave-down square divider and Lead uses its selected divider waveform.
- `noise`: linear deterministic voice-local source level. Chord maps 0–100% to 0–0.35 amplitude for a restrained Juno-style noise source; Lead maps 0–100% to 0–1.0 amplitude so noise can act as a full SH-101 mixer source. Noise is lockable but not LFO-modulatable.
- `cutoff`: maps exponentially from 20 Hz to the lower of 20 kHz or 45% of sample rate.
- `resonance`, and positive `filter envelope` up to six octaves.
- `attack`, `decay`, `sustain`, and `release`; Chord uses approximately 1 ms–3 s attack and 2 ms–12 s decay/release, while Lead uses 1.5 ms–4 s and 2 ms–10 s respectively.

Chord additionally has a stereo `chorus` selector with Off, I, and II modes. Mode I uses approximately 15 ms base delay, 1.5 ms modulation depth, and 0.5 Hz; mode II uses 12 ms, 2.5 ms, and 0.8 Hz. Mode changes crossfade over approximately 5 ms. Overlapping Chord groups keep independent chorus state while following the current shared Chord controls. Chorus precedes post-fader stereo sends.
Chord also has a `spread` selector: Off keeps every voice at the track pan, Narrow uses half stereo width, and Wide uses full width. One voice stays centered, two voices are placed left/right, three voices left/center/right, and four voices left/inner-left/inner-right/right in stored voice order; positions are centered around track pan and clamped at the boundaries. Spread is captured for each chord voice group so release tails retain their layout, is lockable per step, and is not LFO-modulatable. Chorus preserves stereo voice input while remaining centered when spread is Off.

Chord's arpeggiator can be enabled with Up, Down, Up-Down, Down-Up, or Random ordering at `1/32`, `1/16T`, `1/16`, `1/8T`, `1/8`, `1/4T`, or `1/4`. Up-Down and Down-Up omit repeated endpoints, including for two-note shapes. A single-note shape retriggers that note at every arpeggio interval. Random uses deterministic shuffled no-repeat cycles, and each arpeggiated tone retains its original Chord voice-position stereo spread.

Lead additionally provides a `sub mode` selector for one-octave square, two-octave square, or two-octave narrow pulse; 0–100% `keyboard tracking` around C3; and `portamento time`, which maps 1–100% exponentially from 1 ms to 5 seconds while 0% disables glide. These controls are lockable but not LFO-modulatable. AUTO portamento captures the source note's effective time, including inherited tie locks, before applying the target note's locks.

Chord defaults: 70% Saw mix, pulse width 50%, sub 0%, noise 0%, chorus I, cutoff 55%, resonance 15%, filter envelope 25%, and ADSR 55/45/75/65%. Lead defaults: 75% Saw mix, pulse width 50%, sub 25%, noise 0%, two-octave square sub mode, keyboard tracking 50%, portamento time 50%, cutoff 50%, resonance 35%, filter envelope 55%, and ADSR 0/35/55/20%.

The Chord render path uses a stable DCO-style oscillator with additive pulse, saw, octave-down square, and a restrained deterministic voice-local noise source. A fixed 32 Hz high-pass stage precedes its dedicated Juno-calibrated four-pole VCF; separate resonance compensation emphasizes the cutoff region while limiting pass-band loss, maximum resonance is bounded, and output remains finite. Chorus keeps the existing two-mode stereo delay geometry with a calibrated dry-biased equal-power mix.

The Lead render path uses the same phase-aligned primitive only for its common oscillator work, then uses an additive pulse/saw mixer, the selected sub-divider mode, a full-range deterministic voice-local noise source, and a dedicated SH-101-calibrated four-pole VCF. Its separate resonance compensation emphasizes the cutoff region while limiting pass-band loss; maximum resonance is bounded and output remains finite. Keyboard tracking is adjustable from 0–100% around C3, so positive tracking opens the filter on higher notes and closes it on lower notes. Lead ties preserve the gate and ADSR state for legato phrases; ordinary notes retrigger the envelope.

All pitched tracks default to input degree 1 and octave 3. Their oscillators and filters run at 2x oversampling. Chord uses stable DCO pitch and a smoother resonance-compensated response; Lead uses stronger drive and feedback.

### 3.10 Mixer and effects

Each track has a serial distortion-then-phaser-then-flanger chain before its fader, pan, and delay/reverb sends. Distortion drive maps exponentially from unity to approximately 31.6× pre-gain, followed by soft clipping; tone is a 700 Hz–18 kHz low-pass; and mix is dry/wet. The phaser is a four-stage stereo all-pass network with opposite-channel logarithmic modulation: rate maps exponentially from 0.05–8 Hz, depth controls a 300 Hz–8 kHz sweep, feedback is limited to 90%, and mix is dry/wet. The flanger is a stereo fractional-delay network with independent feedback lines and opposite-channel sine modulation: rate maps exponentially from 0.05–8 Hz, center delay maps linearly from 0.2–10 ms, and depth requests a 0–5 ms bipolar excursion. Its effective depth is capped at `center - 0.1 ms`, so the rendered range always remains at or above 0.1 ms without a clamped half-cycle; the UI shows the actual lower and upper range. Feedback is limited to 90%, and mix is dry/wet. Distortion defaults to drive 0%, tone 50%, mix 0%; phaser defaults to rate 25%, depth 50%, feedback 20%, mix 0%; flanger defaults to rate 25%, approximately 2 ms center delay, depth 50%, feedback 20%, mix 0%. All three chains use preallocated state, are separately stateful for live playback and audition, smooth parameter changes, and clear state on Stop and project reset. Silent stages drain until their own output remains below the silence threshold for a stage-appropriate interval, with a two-second safety ceiling; sleeping clears feedback state so old audio cannot reappear. Overlapping Chord groups use independent effect state but follow the same current shared track effect controls and locks.

The track detail panel has `PARAMS` and `EFFECTS` banks. `Tab` toggles banks; the effects bank exposes distortion `d/t/x`, phaser `r/e/f/M`, and flanger `R/q/E/F/N` controls for rate, delay, depth, feedback, and mix. Effects are shared across patterns and support BASE and per-step LOCK values, but not LFO modulation.

Each track provides:

- Level, default 80%
- Mute, default off
- Delay send, default 0%
- Reverb send, default 0% (20% for Chord and Lead)
- Swing, default 0%, range 0–75%, shared across all patterns
- Probability, default 100%, range 0–100%, shared across all patterns

Swing delays only global offbeat sixteenths (clock steps 2, 4, …) by its percentage of the nominal step duration. It applies to the complete per-track action, including releases and locks, and remains aligned to the global clock for polymetric tracks. Conditions are evaluated once at the reached step, followed by the probability gate. A successful event launches its full evenly-spaced retrigger burst before that track's next swing-adjusted slot. Cycle counters, event-Chance streams, and probability streams are deterministic and independent per track; all reset on Stop and pattern activation. Probability draws are not made at 0% or 100%, and never perturb event-Chance streams.

Sends are post-fader and post-mute. Chord group level, pan/spread layout, delay send, and reverb send are captured when a group triggers and remain with that group through release; a later Chord lock cannot reroute an earlier group’s tail. Muting ramps the dry track and new send input to silence, but already-generated global effect tails continue. A muted synth voice continues its internal state, so unmuting may reveal a still-active voice.

#### Kick sidechain ducking

The global Ducking control is a fixed Kick → Bass/Chord/Lead sidechain compressor. It is off by default. Depth is 0–100% and maps to 0–18 dB maximum attenuation; attack maps exponentially from 0.5–30 ms and release maps exponentially from 40–1000 ms. The detector follows the kick after its track effect chain and mute/fader, using a stereo peak envelope with attack/release smoothing. The resulting gain is `10^(-(depth_db × envelope)/20)`.

The shared gain is applied to Bass, Chord, and Lead after their track effects and before their faders, pans, and delay/reverb sends. The kick, drums, preview audition audio, and already-generated delay/reverb returns are not ducked. Detector state continues through live edits and pause and is cleared by Stop and project reset. The `Ducking` card uses shortcut `d`; Enter opens Depth, Attack, and Release fields, Left/Right selects a field, Up/Down changes 1%, Shift changes 10%, and number-row percentage entry edits Depth. Enter/Esc closes the editor and arrow edits remain applied on Esc. Ducking is global-only and cannot be locked, LFO-modulated, or overridden per pattern.

The internal engine renders stereo. A mono output device receives the arithmetic average after master limiting. On devices with more than two channels, channels 1 and 2 receive left and right and additional channels receive silence.

#### Delay

The delay is a tempo-synchronized stereo cross-feedback delay with no independent return-level control. Supported divisions, in selection order, are:

`1/32`, `1/16T`, `1/16`, `1/8T`, `1/8`, `1/8D`, `1/4T`, `1/4`, `1/4D`, `1/2`, and `1 bar`.

Triplet values are two-thirds of their straight counterpart; dotted values are one-and-a-half times their straight counterpart. At the minimum tempo, the implementation must preallocate enough delay memory for the longest supported value. Delay-time or tempo changes crossfade between taps rather than abruptly changing the read position.

Feedback ranges from 0% through 95% to prevent unity or unstable feedback. Default division is `1/8`; default feedback is 30%.

#### Reverb

The reverb is a stereo algorithmic Schroeder/Freeverb-style network using a stereo pre-delay, a fixed 180 Hz stereo high-pass input filter, parallel feedback comb filters, and series all-pass filters. Reverb time specifies the low-frequency RT60. Tone controls the comb damping: 0% is darkest and 100% is brightest. Tone, pre-delay, and return changes are smoothed or crossfaded to avoid clicks. The reverb return is independent from the delay return; delay output is not injected into the reverb input. It has no samples or convolution impulse.

Reverb time ranges from 0.2 through 10.0 seconds and defaults to 2.5 seconds. Reverb tone ranges from 0% through 100% and defaults to 40%. Reverb pre-delay ranges from 0 through 200 ms and defaults to 20 ms. Reverb return ranges from 0% through 100% and defaults to 30%.

#### Master safety

The final output stage applies DC blocking, fixed +6 dB makeup gain, and a stereo-linked limiter with 5 ms lookahead, 1 ms attack, 80 ms release, and a -1 dBFS ceiling. The limiter stores one stereo-linked peak per input frame in a fixed-capacity, preallocated monotonic deque and updates the sliding maximum in amortized O(1); processing and clearing it must not allocate. Ordinary samples are not passed through an always-on waveshaper. Non-finite values are replaced with silence and final conversion clamps to the device format. The limiter is not exposed as a user parameter.

## 4. Global musical parameters

| Parameter | Range | Default | Editing behavior |
| --- | --- | --- | --- |
| Tempo | Integer 40–240 BPM | 120 BPM | Type a complete BPM value and press Enter, or use up/down by 1 and Shift+up/down by 5 |
| Delay time | Supported division list | `1/8` | Up/down moves through the list |
| Delay feedback | 0–95% | 30% | Percentage direct entry and arrows |
| Reverb time | 0.2–10.0 s | 2.5 s | Up/down by 0.1 s and Shift+up/down by 1 s |
| Reverb tone | 0–100% | 40% | Percentage direct entry, or up/down by 1% and Shift+up/down by 10% |
| Reverb pre-delay | 0–200 ms | 20 ms | Up/down by 1 ms and Shift+up/down by 10 ms |
| Reverb return | 0–100% | 30% | Percentage direct entry, or up/down by 1% and Shift+up/down by 10% |
| Key | C, C#, D, D#, E, F, F#, G, G#, A, A#, B | C | Up/down moves chromatically |
| Scale | Major, natural minor | Major | Up/down or the shortcut toggles the value |

Enharmonic keys use sharp names in the MVP. Major uses semitone offsets `[0, 2, 4, 5, 7, 9, 11, 12]`; natural minor uses `[0, 2, 3, 5, 7, 8, 10, 12]`.

Global parameters cannot be parameter-locked.

## 5. Keyboard interaction

The application uses ordinary portable terminal press events. It must not require Kitty keyboard-protocol release events. Mouse capture is not enabled.

### 5.1 Navigation mode

- Up/down moves between physically adjacent sequencer rows, preserving the selected cell's 32-column position. It moves within a track's continuation row when present and otherwise into the adjacent track; unavailable columns clamp to the destination row's final valid step. Vertical navigation clamps at the global row and final track row.
- Up from Track 1's first row selects the global row; down from the global row selects Track 1 step 1. Selecting globals resets parameter scope to `BASE`; moving within or between tracks preserves it.
- On a track, left/right moves the selected step and wraps within its current length.
- Shift+left/right moves between 16-step banks, preserving the within-bank offset when it exists and clamping within a partial final bank.
- On the global row, left/right cycles through global parameters and wraps.
- `Enter` edits the selected global control or toggles/inserts the selected track event as defined in the sequencer model.
- `Backspace` or `Delete` clears the selected event and its locks.
- `Shift+Delete` immediately clears every event and lock from the selected track in the active pattern. It preserves the track length and all non-sequence settings, and is one undoable edit.
- `l` opens numeric track-length input. Digits plus Enter set an exact length; up/down changes it by 1 and Shift+up/down by 16, clamped to 1–64. Arrow changes apply immediately and remain applied on Esc.
- `Esc` exits overlays or parameter editing first; from track navigation it returns lock scope to `BASE`.

### 5.2 Key map

| Context | Key | Action |
| --- | --- | --- |
| Anywhere | `Space` | Play/pause |
| Anywhere | `.` | Stop, reset, and clear effect tails |
| Anywhere | `Ctrl+P` | Open the dynamic pattern dialog |
| Anywhere | `Ctrl+N` | Create a new untitled project, with dirty confirmation |
| Anywhere | `g` | Open the pattern-idea generator |
| Pattern dialog | Left/right, `Home`, `End` | Move the pattern cursor |
| Pattern dialog | `Enter` | Select or queue the cursor pattern and close |
| Pattern dialog | `N` / `D` | Insert an empty pattern / duplicate the cursor pattern |
| Pattern dialog | `C` / `X` / `V` | Copy / cut / paste after the cursor |
| Pattern dialog | `Delete` | Remove the cursor pattern, resetting the final pattern instead |
| Track | `o` | Audition selected track/step without editing |
| Anywhere | `Ctrl+S` | Save, prompting if no current path exists |
| Anywhere | `Ctrl+Shift+S` | Save as |
| Anywhere | `Ctrl+O` | Open project browser |
| Anywhere | `Ctrl+Q` | Quit, with dirty confirmation |
| Anywhere | `Ctrl+Z` | Undo |
| Anywhere | `Ctrl+Y` | Redo |
| Track navigation/parameter screen | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy, cut, or paste the selected step exactly; paste requires the same track kind |
| Navigation, parameter, LFO, or Chord editor | `?` | Open the full help overlay |
| Navigation or parameter editor | `~` (`Shift+\``) | Jump to the global-controls row |
| Track | `p` | Toggle visible `BASE`/`LOCK` scope |
| Track | `Shift+Left` / `Shift+Right` | Move to the previous/next 16-step bank |
| Track | `PageUp` / `PageDown` | Move to the previous/next step while editing a parameter |
| Track | `Shift+1`–`Shift+9` | Jump to the corresponding track |
| Track | `l` | Edit the selected track length from 1 through 64 steps |
| Track | `Shift+D` | Double the selected track by appending an exact copy, when its length is at most 32 |
| Track | `Shift+Delete` | Clear all events and locks from the selected track in the active pattern |
| Trigger, note, or empty step | `A` | Toggle event accent, or the track's input accent default on an empty step |
| Bass/Lead note | `Shift+G` | Toggle slide |
| Trigger/note | `Shift+T` | Edit condition, cycle/chance values, and retrigger count |
| Track | `Shift+S` | Edit 0–75% swing |
| Track | `Shift+Q` | Edit 0–100% probability |
| Eligible parameter editor | `Shift+L` | Add or edit that parameter's track-level LFO |
| Track | `v` | Edit level |
| Track | `m` | Toggle mute immediately |
| Track | `y` | Edit delay send |
| Track | `b` | Edit reverb send |
| Track | `n` | Edit pan |
| Kick | `u` / `d` / `a` | Edit tune, decay, or attack |
| Snare | `u` / `t` / `s` | Edit tune, tone, or snappy |
| Hi-hat | `u` / `d` | Edit tune or decay |
| Tom/Cymbal/Rimshot | `u` / `t` / `d` | Edit tune, tone, or decay |
| Hi-hat | `1` / `2` / `0` | Select Closed/Open recipe, or clear its step overrides |
| Tom | `1` / `2` / `3` / `0` | Select Low/Medium/High recipe, or clear its step overrides |
| Pitched track | `1`–`8` | Insert/replace note at current input octave |
| Pitched track | `[` / `]` | Decrease/increase input octave, clamped to 0–7 |
| Pitched track | `t` | Insert, replace with, or clear a tie subject to validation |
| Bass | `w` / `c` / `R` / `f` | Edit waveform, cutoff, resonance, or filter-envelope amount |
| Bass | `d` | Edit decay |
| Effects bank | `d/t/x` | Edit distortion drive, tone, or mix |
| Effects bank | `r/e/f/M` | Edit phaser rate, depth, feedback, or mix |
| Effects bank | `R/q/E/F/N` | Edit flanger rate, center delay, depth, feedback, or mix |
| Chord/Lead | `w` / `P` / `u` | Edit oscillator mix, pulse width, or sub-oscillator level (`Shift+Q` is track probability) |
| Chord/Lead | `c` / `R` / `f` | Edit cutoff, resonance, or filter-envelope amount |
| Chord/Lead | `a` / `d` / `s` / `r` | Edit ADSR |
| Chord | `h` | Edit chorus Off/I/II mode |
| Chord | `e` | Edit spread Off/Narrow/Wide |
| Chord/Lead | `i` | Select the LFO-only pitch card |
| Chord | `C` | Edit the selected Chord note's shape, or the Chord input shape on an empty step |
| Global | `t` | Edit tempo |
| Global | `y` | Edit delay division |
| Global | `f` | Edit delay feedback |
| Global | `r` | Edit reverb time |
| Global | `b` | Edit reverb tone |
| Global | `p` | Edit reverb pre-delay |
| Global | `m` | Edit reverb return |
| Global | `k` | Edit musical key |
| Global | `s` | Toggle/edit scale |
| Global | `d` | Edit kick sidechain ducking |

Shortcuts are resolved by selected section, so repeated letters do not conflict.

### 5.3 Parameter editing mode

- Pressing a parameter shortcut enters a visibly labelled value editor.
- Pressing another valid parameter shortcut switches the editor to that parameter without leaving the current BASE/LOCK scope.
- Left/right cycles through the selected track's visible parameter controls and wraps at either end. Shift+left/right continues to move between step banks.
- PageUp/PageDown moves to the previous/next step of the current track, wrapping at its length, while keeping the active parameter and BASE/LOCK scope. Shift+1 through Shift+9 jumps to the corresponding track; an incompatible parameter switches to that track's first compatible parameter.
- A number-row percentage assignment applies immediately, keeps the parameter editor open, and ramps the affected continuous control to its new value over approximately 30 ms like a quick fader movement.
- Up/down assignments apply immediately and keep the editor open for repeated changes. On Bass waveform, either direction switches between Saw and Square; on Chord chorus, Up/Down moves through Off, I, and II without wrapping.
- Enter or Esc returns to navigation without reverting changes already made.
- A series of repeated arrow changes to one value is coalesced into one undo transaction until the parameter changes, editing ends, or 300 ms elapses without another adjustment.
- Mute remains a discrete immediate action; Bass waveform and Chord chorus use discrete persistent editors.
- `C` opens a compact horizontal Chord trigger editor over the selected track's parameter section, keeping the sequencer visible. Left/Right selects Shape, Arp, Type, or Rate; Up/Down changes the selected value and stops at list boundaries; PageUp/PageDown moves between steps. Shape order begins with `1`, `1-3`, and `1-5`, followed by the existing three- and four-note shapes. Type and Rate remain remembered but are disabled while Arp is off. Note triggers show their values, ties show inherited source values read-only, and empty steps edit input defaults. Chord settings are not BASE/LOCK parameters.
- `Shift+L` on an eligible parameter immediately creates the default enabled sine, quarter-note, 10%-depth LFO when none exists, then opens its modal editor. Existing assignments open unchanged.
- Chord and Lead show an LFO-only `Pitch LFO` card selected by `i`. It displays assignment depth and its physical bipolar range; it has no BASE value, LOCK value, or direct percentage editor. `Shift+L` opens the same LFO modal for pitch, and Backspace/Delete removes the assignment.
- The LFO modal uses left/right to select enabled, waveform, trigger reset, starting phase, rate mode, rate, or depth; up/down adjusts the selected field, Shift+up/down changes percentage fields by 10, and number-row percentage entry applies to starting phase, free rate, and depth. Enter or Esc closes without reverting immediate edits. Backspace or Delete removes the assignment.
- `A` toggles accent immediately on a trigger or note, or toggles the selected track's persisted input accent default when the step is empty without creating an event. `Shift+G` toggles slide on a Bass or Lead note. `Shift+T` opens the trigger editor; its mode-specific inactive fields remain visibly disabled. `Shift+S` edits selected-track swing with 1% arrows and 10% Shift+arrow changes. `Shift+Q` opens the selected-track probability editor with the same controls: Up/Down changes by 1%, Shift+Up/Down by 10%, and values clamp to 0–100%. Enter, Esc, or Shift+Q closes while retaining immediate edits. These edits are undoable and repeated arrow changes coalesce into one transaction; direct accent editing remains invalid on ties. Lowercase `p` remains the BASE/LOCK scope toggle, and `P` remains the Chord/Lead pulse-width shortcut.

The pattern-idea generator opens with `g` and is session-only; its settings are never written to project JSON. Its fields, in order, are `Target`, `Track`, `Seed`, `Density`, `Low octave`, `High octave`, `Chord shapes`, `Ties`, `Accents`, and `Slides`. Up/down moves between fields and clamps at the first or last field; Tab and BackTab move through the same ten-field order and wrap. Target and Track use left/right, the Track selector wraps through all nine tracks, and Seed accepts digits with Backspace (left also removes its last digit). Percentage fields change by 5 points and clamp to 0–100%. Low octave and High octave use left/right one octave at a time: Low is clamped to 0 through High, while High is clamped to Low through 7. Chord shapes is an ordered selector that clamps through Default, Root shapes, and All shapes. Enter applies the generator and Esc closes it; range edits do not alter existing events. Defaults are the deterministic seed, 48% density, O2–O6, All shapes, 18% ties, 24% accents, and 18% slides. Rimshot generation uses steps 5 and 13 as its higher-probability backbeat anchors.

The generator popup is a centered 58×15 rectangle at the standard terminal size and is capped to smaller terminals. The inclusive octave range controls Bass and Lead note octaves. Chord note roots are independently randomized across that same inclusive range; chord shape tones may then extend above the stored root octave according to the selected shape and inversion. Default shape generation uses only `1-3-5`. Root shapes selects uniformly from `1`, `1-3`, `1-5`, root triad, root seventh, root sixth, root sus2, and root sus4. All shapes selects uniformly from all 20 explicit shapes and inversions. Slides are independently applied only to newly generated Bass and Lead notes. When targeting one track, Chord shapes is inactive outside the Chord track and Slides is inactive outside Bass and Lead; inactive fields remain visible, are marked `n/a`, use muted styling, and ignore value changes. Both fields remain active for Whole pattern generation. Existing events are never changed by generation.

### 5.4 Audition behavior

Adding or replacing a trigger or note automatically auditions it while transport is stopped or paused. The automatic audition is checked when its audio command is consumed, so it is discarded if playback has started in the meantime. Clearing an event never auditions.

`o` explicitly auditions at any transport state without changing transport or pattern data:

- A drum row auditions the selected trigger's accent and locks, or a hit using the track's input accent default with base values on an empty step.
- A Bass/Lead note auditions its pitch, accent, and locks; a Chord note auditions its complete selected shape.
- A tie resolves and auditions its source note and accent while applying effective tie-chain locks.
- An empty pitched step auditions the last-entered note using the track's input accent default with base values.
- A synth audition holds the gate for one quarter note at the current tempo and then enters release.
- Explicit audition may overlap normal sequence playback through independent preview voices and chorus state without altering sequenced voice state.

## 6. Terminal interface

### 6.1 Layout

At `120x34` or larger, the normal screen contains:

1. Header: one metadata-only line containing the application name, project filename or `Untitled`, dirty marker, audio device/status, transport state, pattern state, and tempo. If the current audio stream has had callback deadline overruns, the header also shows a text-visible warning badge with the cumulative count and maximum callback load percentage. Persistent command shortcut hints are not shown there; the `?` overlay contains the complete key map.
2. Global row: all ten global controls and current values; their local shortcuts are shown in the detail cards.
3. Eight variable-length sequencer track blocks: track name, mute state, absolute step range, and compact fixed-width step cells. The displayed range communicates the track length.
4. A selected-control panel: vertical parameter faders for the selected track, or ten titled global detail cards when the global row is selected. The global cards show a tempo numeric readout, delay-division/key/scale selectors, and faders for delay feedback, reverb time, reverb tone, reverb pre-delay, and reverb return. Global faders use their model ranges (`0–95%`, `0.2–10.0 s`, `0–100%`, `0–200 ms`, and `0–100%`) and show exact values with units beside the fader; every card retains its local shortcut.
5. Status line: current mode, last successful operation or actionable error, and active-editor guidance. While a track parameter is being edited in `LOCK` scope, the selected-control panel and status line prominently show `LOCK PARAMETER EDITING` using contrasting styling.

Track percentage parameters use ten vertically stacked segments, filled proportionally and accompanied by an exact percentage. Hi-hat shows adjacent Closed and Open Tune/Decay groups; Tom shows adjacent Low, Medium, and High Tune/Tone/Decay groups. In BASE scope every recipe is editable. In LOCK scope only the selected trigger's recipe group remains active and its locks override that recipe. The selected-track title and non-default step-cell digit identify the referenced recipe. The selected-track title also shows event accent, the empty-step input accent default, inherited accent/source on ties, and Bass or Lead slide state. Bass waveform and Chord chorus use the same column geometry as discrete switches. Mixer, instrument, filter, envelope, distortion, phaser, and flanger groups use distinct colors. The active parameter editor is marked with a heavy outline, reverse styling, and a bold label. In `LOCK` scope, faders show effective values and explicitly identify `LOCK` overrides versus recipe/base-inherited values. Physical units are shown in the active readout; flanger readouts show Hz, center delay/excursion in ms, feedback, and wet mix. A `~` badge marks parameters with an LFO assignment, including disabled assignments. The Chord/Lead `Pitch LFO` card is LFO-only and shows depth plus its ±semitone range instead of a base or lock value.

The compact, centered track-level LFO modal arranges enabled, waveform, trigger reset, starting phase, rate mode, rate, and depth as seven control columns from left to right, matching left/right field selection and up/down value adjustment. Its width is capped at 116 columns and shrinks safely on smaller areas. Control names occupy their card borders to avoid duplicated labels and empty space. Enabled, trigger reset, and rate mode use two-position switches; waveform and synchronized rate use multi-value selectors that fill all available rows; and starting phase, free rate, and depth use ten-segment faders. Up selects the displayed option above and Down selects the option below; switches and selectors stop at their first and last values instead of cycling. For faders, Up increases and Down decreases. Starting phase shows both percentage and derived degrees. The selected column uses the same heavy outline, reverse styling, and bold labeling as an active parameter. Rate shows its synchronized division or free percentage together with the resulting physical Hz value; ordinary depth is labeled in bipolar percentage points, while pitch depth also shows its ±semitone range. The Chord editor uses the same compact treatment as LFO, with four equal-width Shape, Arp, Type, and Rate fields, disabled Type/Rate styling while Arp is off, trigger-origin indicators, and PageUp/PageDown step navigation. The trigger editor uses the same large card treatment with five horizontal Mode, Phase, Length, Chance, and Retrigger fields: Mode, Phase, Length, and Retrigger are multi-option selectors, Chance is a ten-segment percentage fader, and selector arrows follow the displayed order. Inactive mode-specific fields remain visibly muted. Swing and pattern-generator dialogs use compact, content-sized centered overlays.

Parameter shortcuts are displayed beside the controls they operate in the detail cards. Other event, navigation, and track-action shortcut hints are not shown in the main layout. The help overlay remains available for the complete key map; there is no persistent bottom instruction panel.

Step cells use these textual forms:

- `.` empty
- `x` / `X` normal/accented drum trigger
- `D:O` / `D!O` normal/accented note degree `D` at octave `O`
- `D*O` / `D#O` locked normal/accented note
- `-` tie
- `*` additional lock marker

Bass and Lead notes with slide are underlined.

The selected cell and currently playing cell have independent styling. If both refer to the same cell, the combined style must still communicate both states. Mute, event type, and lock state must not rely on color alone.

Each pitched row includes its current input octave in the track label (for example, `Bass O3`).

The sequencer grid uses 32 fixed-width cells per physical line with a visible divider after each 16-step bank. Steps 33 through 64 use a continuation line. Cells beyond a track's length are blank and cannot be selected. The detail panel is only as tall as its faders or global cards require, and all remaining vertical space is assigned to the sequencer. When expanded track blocks still exceed the pattern panel height, the panel scrolls by complete track blocks to keep the selected track visible. Wider terminals do not stretch individual step cells.

When the terminal is smaller than `120x34`, replace the main layout with the current size, required size, and quit/help keys. The project and audio engine remain active so resizing restores the normal view.

### 6.2 Modes and overlays

The current mode is always named on screen. Modes are:

- Navigation
- Pattern dialog
- Parameter, global, LFO, Chord, trigger, and swing editors
- Tempo numeric input
- Track-length input
- Project browser
- Project-name input
- Open, new-project, and quit confirmations
- Error dialog
- Help

The project browser opened by `Ctrl+O` lists all regular, non-temporary files in `.projects/`, sorted by filename. Up/Down selects an entry, Home/End jump to the first/last entry, Enter opens it, and Esc closes the browser. A missing or empty `.projects/` directory is shown as empty. Explicit CLI project paths remain unchanged.
Save As accepts a non-empty single filename component, writes it under `.projects/`, and appends `.groove.json` once if needed. Names containing `/` or `\\`, or equal to `.` or `..`, are rejected. The destination is shown before confirmation; the directory is created lazily on save.

Open and quit with a dirty project present a `Save`, `Discard`, `Cancel` choice. Save failure leaves the current project dirty, shows an error, and clears any pending open/new/quit continuation so a later unrelated Save As cannot trigger it. Opening a project stops and resets playback, clears effects, loads the new engine state, resets undo/redo history, selects the global row, and marks the project clean.

`Ctrl+N` creates a new untitled project without restarting. When the current project is clean, it happens immediately. When it is dirty, the application shows `Save`, `Discard`, and `Cancel`; `Save` uses the current path or opens Save As for an untitled project. Creating a new project stops and resets playback, clears effects, loads the default project, resets undo/redo history, selects the global row, clears the project path, and marks the project clean.

## 7. Undo, redo, and dirty state

- Retain up to 256 project-changing transactions in memory.
- Undoable changes include events, tie cleanup, locks, base parameters, global parameters, waveform, mute, input degree/octave/accent default, and project-wide edits.
- Transport, cursor position, selected section, active mode, status messages, and audition are not undoable.
- A new edit after undo clears redo history.
- Loading a project or creating a new project clears both histories.
- Undo history is never serialized.
- Dirty state is based on whether the current project model equals the last successfully loaded or saved revision. Undoing back to that revision clears the dirty marker; redoing away from it restores the marker.
- An edit must not be committed to the UI model unless its corresponding engine command can be queued. A full engine queue produces a visible error instead of allowing UI/audio state to diverge.

## 8. Project file format

### 8.1 General rules

- Project files are UTF-8, pretty-printed JSON ending with a newline.
- The conventional extension is `.groove.json`. TUI Save As appends this extension to bare names and does not duplicate it when already present; explicit CLI project paths are used literally.
- Version 20 is strict: reject unknown fields, enum values, invalid numeric ranges, incorrect track layouts, top-level track sequences, pattern counts outside 1 through 100, step counts outside 1 through 64, incompatible events/locks/LFOs/recipes, invalid tie graphs, and song references outside the dynamic pattern list. The required `globals.sidechain` object contains `depth`, `attack`, and `release` percentages. Version 19 and earlier files remain rejected; unsupported versions, missing versions, and unknown future versions are rejected without migration.
- A failed load leaves the current project, undo history, dirty state, and engine untouched.
- A successful save writes a temporary sibling file, flushes it, and atomically renames it over the destination. A failed save leaves the previous destination intact and the current project dirty.

### 8.2 Logical schema

The top-level object is:

```json
{
  "format_version": 20,
  "globals": {},
  "tracks": [],
  "patterns": [],
  "song": []
}
```

`globals` contains:

```json
{
  "tempo_bpm": 120,
  "delay_division": "eighth",
  "delay_feedback": 30,
  "reverb_time_seconds": 2.5,
  "reverb_tone": 40,
  "reverb_pre_delay_ms": 20,
  "reverb_return": 30,
  "sidechain": {
    "depth": 0,
    "attack": 20,
    "release": 35
  },
  "key": "C",
  "scale": "major"
}
```

`delay_division` accepts the stable strings `thirty_second`, `sixteenth_triplet`, `sixteenth`, `eighth_triplet`, `eighth`, `eighth_dotted`, `quarter_triplet`, `quarter`, `quarter_dotted`, `half`, and `bar`.

`tracks` contains exactly nine entries in the fixed instrument order. Every track stores:

- `kind`: `kick`, `snare`, `hat`, `tom`, `cymbal`, `rimshot`, `bass`, `chord`, or `lead`
- `name`: the fixed display identifier
- `level`: integer 0–100
- `pan`: integer 0–100; omitted values load as 50
- `muted`: Boolean
- `delay_send`: integer 0–100
- `reverb_send`: integer 0–100
- `swing`: integer 0–75; omitted values load as 0
- `probability`: integer 0–100; omitted values load as 100
- `effects.distortion.drive`, `effects.distortion.tone`, `effects.distortion.mix`: integer 0–100
- `effects.phaser.rate`, `effects.phaser.depth`, `effects.phaser.feedback`, `effects.phaser.mix`: integer 0–100; phaser feedback is limited to 90
- `effects.flanger.rate`, `effects.flanger.delay`, `effects.flanger.depth`, `effects.flanger.feedback`, `effects.flanger.mix`: integer 0–100; flanger feedback is limited to 90
- An `instrument` object with the applicable base values
- A required sparse `lfos` object containing compatible per-destination assignments
- Bass, Chord, and Lead additionally store `input_degree` and `input_octave`. Chord tracks may store `input_chord_shape` and `input_chord_arpeggio`; omitted values mean `1-3-5` and disabled/Up/`1/16`.
- Every track may store `input_accent`; omitted means `false`. It is the persisted accent inherited by newly entered triggers and notes and by empty-step audition.

Top-level tracks contain shared configuration only. Sequence data is stored under `patterns[].tracks[].steps`; each pattern track contains 1 through 64 steps, and its array length is the track length.

Hi-hat stores recipe-1 `tune` and `decay` plus a required `open` object with the same fields. Tom stores recipe-1 `tune`, `tone`, and `decay` plus required `medium` and `high` objects with the same fields. Cymbal and Rimshot each store `tune`, `tone`, and `decay`. Chord instruments store `oscillator_mix`, `pulse_width`, `sub_oscillator`, `noise`, `chorus`, `spread`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `decay`, `sustain`, and `release`. `spread` accepts `off`, `narrow`, or `wide`. Lead stores `oscillator_mix`, `pulse_width`, `sub_oscillator`, `noise`, `sub_mode`, `keyboard_tracking`, `portamento_time`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `decay`, `sustain`, and `release`; `sub_mode` accepts `one_octave_square`, `two_octave_square`, or `two_octave_narrow_pulse`. Bass retains `waveform`, `cutoff`, `resonance`, `filter_envelope`, and `decay`.

An empty step is JSON `null`. Populated steps use tagged `trigger`, `bass_note`, `note`, `lead_note`, or `tie` objects with a required `locks` object. `accent` is required and Boolean on triggers and notes; `slide` is additionally required on `bass_note` and `lead_note`; both are invalid on ties. Hi-hat triggers accept recipe 1–2 and Tom triggers 1–3; recipe 1 is omitted, while recipes are invalid on other tracks.

Chord notes may include an optional `chord_shape` string. Omitted values mean `triad_root` (`1-3-5`). They may also include `arpeggio` with `enabled`, `type`, and `rate`; omitted arpeggio means disabled, Up, and `1/16`, while non-default type/rate values remain stored when disabled. The stable shape names are `single`, `dyad_third`, `dyad_fifth`, `triad_root`, `triad_first_inversion`, `triad_second_inversion`, `seventh_root`, `seventh_first_inversion`, `seventh_second_inversion`, `seventh_third_inversion`, `sixth_root`, `sixth_first_inversion`, `sixth_second_inversion`, `sixth_third_inversion`, `sus2_root`, `sus2_first_inversion`, `sus2_second_inversion`, `sus4_root`, `sus4_first_inversion`, and `sus4_second_inversion`. Chord data is invalid on Lead notes.

For example, a Chord note may contain `degree`, `octave`, `accent`, `chord_shape`, `arpeggio`, and `locks`; a tie contains only `locks`.

The `locks` object is always present on populated steps and contains only overridden values. Lock keys use the stable names `level`, `delay_send`, `reverb_send`, `distortion_drive`, `distortion_tone`, `distortion_mix`, `phaser_rate`, `phaser_depth`, `phaser_feedback`, `phaser_mix`, `flanger_rate`, `flanger_delay`, `flanger_depth`, `flanger_feedback`, `flanger_mix`, `tune`, `tone`, `snappy`, `decay`, `waveform`, `oscillator_mix`, `pulse_width`, `sub_oscillator`, `noise`, `sub_mode`, `keyboard_tracking`, `portamento_time`, `chorus`, `spread`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `sustain`, and `release`, subject to track compatibility. `pitch` is not a lock key. Chord chorus values are `off`, `i`, and `ii`; arpeggio settings are note-trigger values, not lock values. `mute`, `accent`, and `slide` are invalid in a lock object.

An empty LFO collection is `{}`. Assignment keys are compatible continuous instrument parameters plus `level`; Chord and Lead may additionally use the LFO-only `pitch` key. Bass waveform, Chord chorus, Chord/Lead Noise, Lead Keyboard Tracking, and mixer sends are excluded. Incompatible assignments are rejected. A synchronized assignment is:

```json
"lfos": {
  "cutoff": {
    "enabled": true,
    "waveform": "sine",
    "reset_on_trigger": false,
    "start_phase": 0,
    "rate": { "mode": "synced", "division": "quarter" },
    "depth": 10
  }
}
```

For Chord or Lead pitch:

```json
{
  "lfos": {
    "pitch": {
      "enabled": true,
      "waveform": "triangle",
      "reset_on_trigger": true,
      "start_phase": 25,
      "rate": { "mode": "synced", "division": "quarter" },
      "depth": 100
    }
  }
}
```

The pitch assignment's `depth` is percentage control; its physical range is `±(depth / 100 * 2)` semitones. Pitch assignments on Bass, drums, or other ineligible destinations fail strict validation. Trigger reset and starting phase are required LFO fields in format version 20.

A free rate uses `{ "mode": "free", "rate_percent": 50 }`. Waveform names are `sine`, `triangle`, `square`, `saw`, and `sample_and_hold`. Synchronized division names are `four_bars`, `two_bars`, `bar`, `half`, `quarter_dotted`, `quarter`, `quarter_triplet`, `eighth_dotted`, `eighth`, `eighth_triplet`, `sixteenth`, `sixteenth_triplet`, and `thirty_second`.

The Rust model must validate these domain concepts rather than untyped maps. There is no stable public Rust API in the MVP; the compatibility interfaces are the CLI, keyboard behavior, and JSON schema.

## 9. Command-line interface

```text
terminal-groove [PROJECT] [--audio-device <exact-name>] [--audio-buffer <frames>]
terminal-groove --list-audio-devices
terminal-groove --help
terminal-groove --version
```

- With no project argument, start a new untitled project using all defaults.
- With a project argument, validate the entire project before entering the TUI.
- The default output device is used unless `--audio-device` is given.
- The output callback requests an automatic 512-frame buffer, clamped to the selected
  device's supported range. `--audio-buffer <frames>` requests an exact supported
  buffer size for low-latency tuning; zero and unsupported values are rejected. If a
  device reports no buffer range, automatic mode retains the device default.
- `--list-audio-devices` prints available output-device names and exits without entering raw terminal mode.
- An audio-device override requires one unique exact name. No match or multiple identical matches produces an error and lists candidates.
- Failure to open the requested output device or build its stream exits nonzero with an actionable message.
- Startup validation and audio errors occur before alternate-screen entry when possible.
- Normal exit, recoverable error exit, and panic must restore raw mode, cursor visibility, and the original terminal screen.

## 10. Technical architecture

### 10.1 Toolchain and dependencies

Baseline:

- Rust 2024 edition
- Minimum supported Rust version 1.85
- Ratatui 0.30 with its Crossterm 0.29 backend
- Crossterm 0.29 for raw mode and portable press events
- CPAL 0.17 for audio output
- `rtrb` 0.3 for real-time-safe SPSC communication
- Serde 1 and Serde JSON 1 for persistence
- Clap 4 derive API for CLI parsing
- A structured error crate such as `thiserror`; application-level context may use `anyhow`

Linux setup documentation must include Rust installation and:

- Debian/Ubuntu: `libasound2-dev`
- Fedora: `alsa-lib-devel`

Install Rust and the ALSA development package before building on a new Linux system. The repository's current toolchain and dependency versions are listed above and are verified by the build and test commands in `AGENTS.md`.

### 10.2 Package organization

Use one binary package with testable modules for model/validation, reducer and history, persistence, TUI, sequencing, DSP/offline rendering, and CPAL integration. Keep the model, reducer, and DSP independent of Ratatui and CPAL.

### 10.3 Threading and real-time rules

- The main thread owns terminal input, rendering, dialogs, undo/redo, file I/O, and the canonical editable project.
- CPAL's audio callback owns transport timing, a mirrored engine project, voices, filters, effects, and sample conversion.
- UI-to-audio communication uses a preallocated bounded SPSC queue of typed commands. Transport, pattern selection, and audition commands are small; project edits are converted on the main thread into immutable boxed project snapshots before being queued.
- Independent per-track playheads and transport telemetry return through atomics or a second bounded channel where intermediate redundant playhead updates may be dropped.
- Each non-empty CPAL callback measures elapsed monotonic time from before command draining/rendering through completion. Its deadline is the current output frame count divided by the selected sample rate, not a hard-coded buffer size. The callback records maximum duration in nanoseconds and maximum elapsed/deadline load in per-mille relaxed atomics, and increments a relaxed cumulative overrun counter only when elapsed time is strictly greater than that deadline. These diagnostics are allocation-free, lock-free, non-blocking, and free of callback formatting or logging; all three counters reset when a new stream is opened.
- Project files are parsed and validated on the main thread. Opening or creating a project queues a stop followed by its immutable snapshot; the callback applies the snapshot at a command boundary and reuses its preallocated audio state.
- The callback must not allocate or free heap-backed project snapshots, lock a mutex, block, access the filesystem, format text, or log. Replaced snapshots are returned through a bounded retirement queue and reclaimed on the main thread.
- Noise generators use preallocated deterministic PRNG state local to each voice.
- Idle drum voices do not advance oscillators, filters, or noise state; their mixer
  smoothers continue advancing so live parameter changes remain synchronized.
- Preview rendering has an explicit per-track activity gate. It includes non-idle preview envelopes, chord/arpeggio voices, scheduled retriggers, chorus state, and insert-effect feedback tails. Inactive preview tracks skip voice mixing, panning, effect processing, and LFO advancement; preview LFO phases remain independent from live phases and reset on Stop or explicit audition.
- Track effect chains bypass all DSP when every wet mix is settled at zero. Chorus,
  delay, and reverb use allocation-free activity gates: silent processors are skipped,
  and active processors continue for their preallocated effect tails before sleeping.
- Insert effects gate distortion, phaser, and flanger independently. Their static mappings, modulation-rate conversions, feedback coefficients, and delay scaling are cached while parameters are settled; phaser all-pass coefficients update in eight-frame control blocks with per-sample interpolation while smoothers and phase continue at audio rate. Effect tails use a bounded, allocation-free drain when input becomes silent. Ordinary live and preview chains exist only for non-Chord tracks; the two live and two preview Chord groups retain independent chains. Flanger channels are preallocated for the documented 15 ms maximum delay at the selected sample rate.
- Synth cutoff targets and modulated envelope coefficients refresh at a bounded eight-sample control rate; filter coefficients and equal-power oscillator/pan gains interpolate between control points. Sidechain attack/release coefficients and LFO rate/smoothing constants are cached, and one sidechain gain is shared by all ducked voices for each output frame.
- The repository includes an ignored diagnostic saturated fixture: `cargo test --release audio::tests::saturated_callback_benchmark -- --ignored --nocapture`. For each 128-, 256-, and 512-frame buffer at 44.1, 48, and 96 kHz, it runs five trials with 128 warm-up and 512 measured callbacks, pools the samples, and reports median, p95, p99, maximum, and median nanoseconds/frame. The supported reference-machine completion target is p95 callback load no higher than 50% for every 44.1/48 kHz configuration. Results at 96 kHz are measured best effort and do not gate completion. Timing has no automated assertion because it is host-dependent; automated tests enforce correctness and callback allocation safety.
- Queue exhaustion is handled on the UI side before committing the model change.
- CPAL stream errors are forwarded to the UI through a non-blocking error path and shown prominently.

### 10.4 Audio format and scheduling

- Use the selected device's default output configuration and sample rate, requesting the
  automatic or explicitly selected callback buffer described in section 9.
- Support the common `f32`, `i16`, and `u16` device sample formats; reject other formats clearly in the Linux-first MVP.
- Render all DSP as `f32` stereo internally and convert only at the final device boundary.
- Allocate voices, filter state, delay memory, and reverb buffers before stream playback.
- Drain pending edits at the beginning of each callback. Step-affecting edits received before a step boundary apply at that boundary.
- UI polling should add no more than approximately 8 ms of avoidable input latency while still allowing efficient redraws.

## 11. Error handling

- Invalid user edits are nonfatal and produce a concise status message.
- Project parse/validation errors identify the JSON path or domain field when possible.
- Audio initialization errors identify the selected device and remediation, including the list-devices command.
- Runtime stream failure stops transport and presents a persistent error. Project editing and saving remain available if terminal operation is still safe.
- Runtime stream failures, audio initialization failures, and DSP non-finite diagnostics are appended to `terminal-groove-audio.log` in the working directory. The UI status names the file; logging is best effort and never occurs in the output callback.
- DSP must replace any unexpected non-finite intermediate or sample with zero before it reaches the device and surface a diagnostic outside the callback.
- Terminal cleanup uses RAII and a panic hook so the shell is not left in raw mode.

## 12. Testing and acceptance

### 12.1 Unit and reducer tests

Cover musical degree/frequency mapping, input limits, tie creation/resolution/cleanup, Bass and Lead gates, Chord shapes and eight-voice overlap, retriggers, ties, releases, accents, audition, and Bass and Lead slide behavior. Cover probability defaults and bounds, undo/redo/coalescing, dirty revisions, condition-before-probability ordering, full-burst gating, pitched-note release behavior, tie immunity, deterministic reset, and independence from event-Chance RNG. Also cover lock overlay/compatibility, percentage editing, tempo accumulation, independent cycles, resize/doubling, delay divisions, track-effect parameter bounds, and LFO compatibility, rates, phase, clamping, pitch range, ties, and release tails.

### 12.2 Persistence tests

Round-trip default and populated version-20 projects, including drum recipes, Rimshot, sidechain, track probability, every event, lock, LFO, effect, flanger setting, and articulation variant. The sidechain object and all three fields are required; missing or invalid values are rejected. Missing `tracks[].probability` loads as 100%; new saves always include it. Reject version 19 and other unsupported versions, incompatible recipes, Noise and Keyboard Tracking LFO assignments, unknown fields, invalid percentages and other ranges/layouts/events/locks/LFOs/ties, and malformed sequences; preserve the active project on load failure; and verify atomic-save failure behavior, dirty-state updates, history reset on load, and exact same-kind step clipboard behavior.

### 12.3 TUI tests

Use Ratatui `TestBackend` at `120x34` and larger to cover fixed-width grids, continuation rows, scrolling, navigation, length/doubling controls, small terminals, faders, switches, readouts, shortcuts, cursor/playhead styling, non-color indicators, BASE/LOCK scope, parameter precedence, event articulation, LFO/pitch-LFO/Chord editors, dialogs, confirmations, help, and terminal restoration.

### 12.4 DSP tests

Cover bounded oscillator pitch, ADSR timing, filter stability, finite drum output, lock smoothing, LFO waveform/rate/phase behavior, pitch modulation, source-owned Lead portamento, Chord group reuse, Bass contour cleanup, delay/reverb/flanger timing and feedback-tail decay, limiter ceiling and makeup gain, sample conversion, deterministic offline rendering, and callback-path allocation safety under active DSP and command transitions.

### 12.5 Manual acceptance scenarios

1. Start an untitled project in a `120x34` terminal; navigate every row, enter events, and verify visible state, local shortcuts, playhead/cursor styling, and small-terminal behavior.
2. Build a drum loop; edit all drum parameters, accents, mute, sends, conditions, retriggers, swing, probability, and locks while stopped and playing. Verify 0% suppresses drums and retriggers, 100% preserves behavior, conditions are evaluated first, and pitched probability failures release active voices while ties remain held.
3. Enter Bass, Chord, and Lead notes with octave changes, shapes, inversions, arpeggiation, accents, slide, ordinary and wrapped ties; verify gates, releases, inherited articulation, and the fixed-time Bass glide.
4. Edit base values, locks, synced/free LFOs, and all track effects including flanger center delay/depth; verify faders, readouts, badges, modulation centers, smoothing, and next-pass live updates.
5. Audition empty and occupied steps with `o` while stopped and playing, including Chord shapes; change key and scale and verify existing degrees are reinterpreted on future triggers.
6. Exercise undo/redo, step copy/cut/paste, tie cleanup, coalesced parameter and recipe edits, dirty restoration, sidechain editing and version-20 save/load with all event, lock, LFO, effect, mixer, articulation, and input settings; verify version-19 rejection.
7. List devices, use the default output, select a unique explicit device, and play for at least ten minutes at a supported low-latency configuration without stream errors, non-finite output, timing drift, or audible edit clicks.
8. Exit normally and simulate startup/runtime failures, confirming that the terminal is always restored and project editing remains safe where supported.

## 13. MVP completion criteria

The MVP is complete when all automated tests pass and every manual acceptance scenario succeeds on a current Linux desktop using ALSA directly or through the system's configured sound-server bridge. The implementation must match the documented keyboard map and JSON schema, generate all sound procedurally, keep audio scheduling independent from TUI redraw timing, and leave no hidden editing mode or silent data-loss path.
