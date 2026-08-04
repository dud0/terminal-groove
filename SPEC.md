# terminal-groove MVP specification

Status: implementation-ready MVP specification  
Target: Linux-first terminal application  
Application and executable name: `terminal-groove`

## 1. Product definition

`terminal-groove` is a real-time groovebox operated entirely from a terminal. Its primary design goals are a fast keyboard workflow, predictable state transitions, and a transparent interface: the selected section and step, transport state, active editing mode, parameter values, triggers, notes, ties, locks, available shortcuts, dirty state, and audio errors must remain visible.

The project has six independently looping track sequences on a shared sixteenth-note clock. Each track contains 1 through 64 steps and defaults to 16 steps, allowing polymetric patterns while retaining a fixed 4/4 timing reference.

There are exactly six tracks in this fixed order:

1. Kick drum
2. Snare drum
3. Hi-hat
4. Bass
5. Chord
6. Lead

All sound is synthesized in real time. The application contains no audio samples.

### 1.1 MVP capabilities

- Start, pause, resume, stop, and reset pattern playback.
- Edit the pattern while it is playing. An edit affects playback the next time the affected step is reached.
- Add drum triggers, synth notes, synth ties, and per-step parameter locks.
- Set per-track level, mute, delay send, and reverb send.
- Add independent LFO modulation to eligible instrument parameters and track level.
- Configure tempo, delay time and feedback, reverb time, musical key, and major or natural-minor scale.
- Audition sounds without modifying the pattern.
- Edit and navigate up to 100 independent patterns.
- Undo and redo project edits within the current session.
- Save and load versioned, human-readable JSON projects.
- Select the system default audio output or an explicit output device from the command line.

### 1.2 Explicitly excluded from the MVP

- Samples, sample import, or sample playback
- MIDI input, output, or clock sync
- WAV or other audio export
- Pattern chaining or song mode
- Time-signature changes
- Swing, microtiming, probability, or continuously variable event velocity
- Polyphonic note entry outside the fixed Chord shape mapping, or oscillator detune
- Per-track pan, solo, master-volume control, or configurable effect returns
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
- `.` stops playback, resets the next step to step 1, releases all voices, and clears delay and reverb state.
- The edit cursor is independent from the playhead.
- Tempo changes become active at the next step boundary without skipping or repeating a step.
- Changing a track length while playing preserves its next local step when that step remains in range; otherwise that track wraps to step 1. Other tracks do not restart.
- Growing a track appends empty steps. Shrinking truncates removed steps immediately and clears ties made invalid by the new cyclic boundary. The complete resize and tie cleanup are one undoable edit.
- Doubling a track of 1 through 32 steps appends an exact copy of all existing steps, including events and locks. Tracks longer than 32 steps cannot be doubled because the result would exceed 64.

### 2.2 Patterns

- A project contains 100 pattern slots, numbered P1 through P100. Each pattern owns the six track sequences; instrument, mixer, global, and effect settings are shared.
- `Ctrl+PageUp` and `Ctrl+PageDown` select the previous or next pattern and wrap at the ends of the bank. `Home` selects P1. `End` selects the highest-numbered pattern containing an event or a non-default sequence length, falling back to P1 when all patterns are empty.
- Pattern selection while stopped is immediate. While playing, the selected pattern is queued for the next bar.
- Existing version-7 projects with fewer than 100 patterns are extended with empty pattern slots when loaded. Version 6 projects are migrated to version 7 as before.

### 2.3 Drum events

A drum step is either empty or contains one trigger with a required Boolean `accent`. New triggers are unaccented. Pressing `Enter` on an occupied drum step clears the trigger, its accent, and all locks.

Each drum track is a single retriggerable synthesized voice. Accent has a fixed engine-specific response: it raises level and also strengthens the kick transient, snare excitation, or hi-hat brightness. Instrument parameters are captured when the hit starts and remain part of that hit's tail. Mixer level and sends continue to follow each sequencer step.

### 2.4 Synth events

A synth step is exactly one of:

- Empty
- A note containing a scale degree from 1 through 8, an input octave from 0 through 7, and a Boolean accent
- A tie

Degree 1 is the root in the stored input octave. Degrees 2 through 7 use the selected scale, and degree 8 is the root one octave above degree 1. Pitch uses twelve-tone equal temperament with A4 = 440 Hz.

Notes are stored as scale degree and octave rather than absolute pitch. Changing the global key or scale therefore reinterprets all existing pattern notes the next time they trigger. A note that is already sounding keeps its current pitch until a new note event starts; a tie does not retune it.

Bass and Lead are monophonic. Chord interprets the stored degree as the root of a selected diatonic shape. The available shapes are `1-3-5`, `1-3-5-7`, `1-3-5-6`, `1-2-5`, and `1-4-5`, plus their cyclic inversions. A shape is stored on each Chord note as an ordered scale-degree recipe, so its musical quality follows the current root and scale. For example, `1-3-5-7` can produce different seventh qualities depending on the root and scale.

- A Bass or Lead note sets pitch, captures its accent, opens the gate, and retriggers its amplitude envelope.
- A Chord note renders its selected shape in close position. The stored octave is the root octave; when an inversion recipe wraps from a higher degree to a lower degree, the wrapped tone rises by one scale octave.
- Chord has two alternating four-voice groups. A new note releases the previous shape and retriggers all of its tones, including common tones; one released shape may overlap before the oldest group is reused.
- A following tie keeps the mono voice or complete chord shape open without retriggering pitch, accent, or envelopes. A tie inherits the source note's Chord shape.
- A following empty step closes the gate and begins release.
- A following note closes/restarts the existing voice at the new pitch, with click-safe envelope handling.
- Bass notes additionally store a Boolean slide. A slide remains armed through ties and glides to the next Bass note over a fixed 60 ms without retriggering its main envelope. An empty step clears it.

Pressing a degree key replaces any existing event on the selected step with that note and preserves compatible locks, articulations, and the Chord shape of an existing Chord note. New notes are unaccented and new Bass notes have slide disabled. Pressing `Enter` on an empty pitched step inserts the track's last-entered degree and octave; empty Chord steps also use the track's last-entered Chord shape. Pressing `Enter` on a note or tie clears it and its locks.

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
- Instrument parameters, waveform, level, delay send, and reverb send are lockable.
- Mute is never lockable.
- At each boundary, the engine computes effective values by overlaying the current step's locks on the base values.
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

Track level maps 0% to silence and 100% to unity gain using a smooth perceptual curve. Effect sends map 0% to no send and 100% to the full post-fader track signal. Tracks are centered in the stereo field.

All continuous DSP parameters use short smoothing ramps. A default ramp of approximately 5 ms is sufficient except for delay-time changes, which require a longer click-free crossfade.

### 3.2 Per-parameter LFOs

Each track may attach one independent LFO to each eligible continuous instrument parameter and track level. Waveform, mute, delay send, reverb send, pitch, accent, slide, and global parameters are not eligible.

Each assignment stores enabled state, waveform, rate, and depth. Waveforms are sine, triangle, square, rising saw, and deterministic sample-and-hold. Sine and triangle begin at the center and rise, square begins high, saw begins at -1 and rises, and sample-and-hold selects one deterministic pseudorandom bipolar value per cycle.

Depth is 0–100 percentage points and is bipolar. The engine adds `waveform * depth` to the current effective base-or-lock percentage, clamps to 0–100, and only then maps the percentage to its physical value. Discontinuous waveforms receive an approximately 5 ms smoothing stage.

Free rate uses an exponential 0–100 control mapping from 0.01 Hz through 20 Hz. Tempo-synchronized cycle lengths, from slowest to fastest, are four bars, two bars, one bar, half, dotted quarter, quarter, quarter triplet, dotted eighth, eighth, eighth triplet, sixteenth, sixteenth triplet, and thirty-second.

Sequence LFO phase starts at zero, resets on explicit Stop, freezes on pause, and continues on resume. Events do not retrigger it. Stopped auditions use independent preview LFO state beginning at phase zero. Drum instrument parameters sample their modulated values when a hit starts; level and pitched-voice destinations modulate continuously, including ties and release tails.

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

The hi-hat remains sample-free: six band-limited square oscillators at fixed inharmonic ratios mix with deterministic noise, then pass through high-pass/band-pass shaping and coarse nonlinear coloration.

- `tune`: moves the metallic source base from approximately 310–670 Hz and its filter bands.
- `decay`: maps exponentially from approximately 25–800 ms, spanning closed to open behavior.

Defaults: tune 50%, decay 20%. Accent adds about 3 dB and a short brightness boost.

### 3.6 Bass, Chord, and Lead

The Bass track is a 303-inspired engine with:

- Band-limited asymmetric saw or square waveform.
- A nonlinear four-stage resonant low-pass filter processed at 2x oversampling.
- `cutoff` mapped exponentially from 80 Hz to 8 kHz, `resonance`, positive `filter envelope` up to five octaves, and `decay` mapped from about 80 ms to 2 seconds.
- Fixed fast amplitude attack/release. Accent adds about 3 dB and a dedicated filter-envelope contour.

Defaults: saw, cutoff 45%, resonance 55%, filter envelope 65%, decay 40%.

Chord is a Juno-60-inspired polyphonic engine; Lead is an SH-101-inspired monophonic engine. Both provide phase-aligned band-limited Pulse and Saw sources, an octave-down square sub oscillator, a nonlinear four-stage resonant low-pass filter, one ADSR for amplitude and positive filter modulation, and these controls:

- `oscillator mix`: 0% is Pulse, 100% is Saw, with an equal-power blend between them.
- `pulse width`: maps from 5% through 95% duty cycle.
- `sub oscillator`: linear octave-down source level.
- `cutoff`: maps exponentially from 20 Hz to the lower of 20 kHz or 45% of sample rate.
- `resonance`, and positive `filter envelope` up to six octaves.
- `attack`, `decay`, `sustain`, and `release`; Chord uses approximately 1 ms–3 s attack and 2 ms–12 s decay/release, while Lead uses 1.5 ms–4 s and 2 ms–10 s respectively.

Chord additionally has a stereo `chorus` selector with Off, I, and II modes. Mode I uses approximately 15 ms base delay, 1.5 ms modulation depth, and 0.5 Hz; mode II uses 12 ms, 2.5 ms, and 0.8 Hz. Mode changes crossfade over approximately 5 ms. Chorus precedes post-fader stereo sends.

Chord defaults: 70% Saw mix, pulse width 50%, sub 35%, chorus I, cutoff 55%, resonance 15%, filter envelope 25%, and ADSR 55/45/75/65%. Lead defaults: 75% Saw mix, pulse width 50%, sub 25%, cutoff 50%, resonance 35%, filter envelope 55%, and ADSR 0/35/55/20%.

All pitched tracks default to input degree 1 and octave 3. Their oscillators and filters run at 2x oversampling. Chord uses stable DCO pitch and a smoother resonance-compensated response; Lead uses stronger drive and feedback.

### 3.7 Mixer and effects

Each track provides:

- Level, default 80%
- Mute, default off
- Delay send, default 0%
- Reverb send, default 0%

Sends are post-fader and post-mute. Muting ramps the dry track and new send input to silence, but already-generated global effect tails continue. A muted synth voice continues its internal state, so unmuting may reveal a still-active voice.

The internal engine renders stereo. A mono output device receives the arithmetic average after master limiting. On devices with more than two channels, channels 1 and 2 receive left and right and additional channels receive silence.

#### Delay

The delay is a tempo-synchronized stereo cross-feedback delay with no independent return-level control. Supported divisions, in selection order, are:

`1/32`, `1/16T`, `1/16`, `1/8T`, `1/8`, `1/8D`, `1/4T`, `1/4`, `1/4D`, `1/2`, and `1 bar`.

Triplet values are two-thirds of their straight counterpart; dotted values are one-and-a-half times their straight counterpart. At the minimum tempo, the implementation must preallocate enough delay memory for the longest supported value. Delay-time or tempo changes crossfade between taps rather than abruptly changing the read position.

Feedback ranges from 0% through 95% to prevent unity or unstable feedback. Default division is `1/8`; default feedback is 30%.

#### Reverb

The reverb is a stereo algorithmic Schroeder/Freeverb-style network using a stereo pre-delay, parallel feedback comb filters, and series all-pass filters. Reverb time specifies the low-frequency RT60. Tone controls the comb damping: 0% is darkest and 100% is brightest. Tone and pre-delay changes are smoothed or crossfaded to avoid clicks. It has no samples, convolution impulse, or independent return-level control.

Reverb time ranges from 0.2 through 10.0 seconds and defaults to 2.5 seconds. Reverb tone ranges from 0% through 100% and defaults to 50%. Reverb pre-delay ranges from 0 through 200 ms and defaults to 20 ms.

#### Master safety

The final output stage applies DC blocking, fixed +6 dB makeup gain, and a stereo-linked limiter with 5 ms lookahead, 1 ms attack, 80 ms release, and a -1 dBFS ceiling. Ordinary samples are not passed through an always-on waveshaper. Non-finite values are replaced with silence and final conversion clamps to the device format. The limiter is not exposed as a user parameter.

## 4. Global musical parameters

| Parameter | Range | Default | Editing behavior |
| --- | --- | --- | --- |
| Tempo | Integer 40–240 BPM | 120 BPM | Type a complete BPM value and press Enter, or use up/down by 1 and Shift+up/down by 5 |
| Delay time | Supported division list | `1/8` | Up/down moves through the list |
| Delay feedback | 0–95% | 30% | Percentage direct entry and arrows |
| Reverb time | 0.2–10.0 s | 2.5 s | Up/down by 0.1 s and Shift+up/down by 1 s |
| Reverb tone | 0–100% | 50% | Percentage direct entry, or up/down by 1% and Shift+up/down by 10% |
| Reverb pre-delay | 0–200 ms | 20 ms | Up/down by 1 ms and Shift+up/down by 10 ms |
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
- `l` opens numeric track-length input. Digits plus Enter set an exact length; up/down changes it by 1 and Shift+up/down by 16, clamped to 1–64. Arrow changes apply immediately and remain applied on Esc.
- `Esc` exits overlays or parameter editing first; from track navigation it returns lock scope to `BASE`.

### 5.2 Key map

| Context | Key | Action |
| --- | --- | --- |
| Anywhere | `Space` | Play/pause |
| Anywhere | `.` | Stop, reset, and clear effect tails |
| Anywhere | `Ctrl+PageUp` / `Ctrl+PageDown` | Select the previous/next pattern, wrapping across the 100-pattern bank |
| Anywhere | `Home` | Select Pattern 1 |
| Anywhere | `End` | Select the highest-numbered pattern containing events or a non-default length |
| Anywhere | `o` | Audition selected track/step without editing |
| Anywhere | `Ctrl+S` | Save, prompting if no current path exists |
| Anywhere | `Ctrl+Shift+S` | Save as |
| Anywhere | `Ctrl+O` | Open project |
| Anywhere | `Ctrl+Q` | Quit, with dirty confirmation |
| Anywhere | `Ctrl+Z` | Undo |
| Anywhere | `Ctrl+Y` | Redo |
| Anywhere | `?` | Toggle full help overlay |
| Track | `p` | Toggle visible `BASE`/`LOCK` scope |
| Track | `Shift+Left` / `Shift+Right` | Move to the previous/next 16-step bank |
| Track | `PageUp` / `PageDown` | Move to the previous/next step while editing a parameter |
| Track | `Shift+1`–`Shift+6` | Jump to the corresponding track |
| Track | `l` | Edit the selected track length from 1 through 64 steps |
| Track | `Shift+D` | Double the selected track by appending an exact copy, when its length is at most 32 |
| Trigger or note | `Shift+A` | Toggle accent |
| Bass note | `Shift+G` | Toggle slide |
| Eligible parameter editor | `Shift+L` | Add or edit that parameter's track-level LFO |
| Track | `v` | Edit level |
| Track | `m` | Toggle mute immediately |
| Track | `y` | Edit delay send |
| Track | `b` | Edit reverb send |
| Kick | `u` / `d` / `a` | Edit tune, decay, or attack |
| Snare | `u` / `t` / `s` | Edit tune, tone, or snappy |
| Hi-hat | `u` / `d` | Edit tune or decay |
| Pitched track | `1`–`8` | Insert/replace note at current input octave |
| Pitched track | `[` / `]` | Decrease/increase input octave, clamped to 0–7 |
| Pitched track | `t` | Insert, replace with, or clear a tie subject to validation |
| Bass | `w` / `c` / `R` / `f` | Edit waveform, cutoff, resonance, or filter-envelope amount |
| Bass | `d` | Edit decay |
| Chord/Lead | `w` / `Shift+P` / `u` | Edit oscillator mix, pulse width, or sub-oscillator level |
| Chord/Lead | `c` / `R` / `f` | Edit cutoff, resonance, or filter-envelope amount |
| Chord/Lead | `a` / `d` / `s` / `r` | Edit ADSR |
| Chord | `h` | Edit chorus Off/I/II mode |
| Chord | `C` | Edit the selected Chord note's shape, or the Chord input shape on an empty step |
| Global | `t` | Edit tempo |
| Global | `y` | Edit delay division |
| Global | `f` | Edit delay feedback |
| Global | `r` | Edit reverb time |
| Global | `b` | Edit reverb tone |
| Global | `p` | Edit reverb pre-delay |
| Global | `k` | Edit musical key |
| Global | `s` | Toggle/edit scale |

Shortcuts are resolved by selected section, so repeated letters do not conflict.

### 5.3 Parameter editing mode

- Pressing a parameter shortcut enters a visibly labelled value editor.
- Pressing another valid parameter shortcut switches the editor to that parameter without leaving the current BASE/LOCK scope.
- Left/right cycles through the selected track's visible parameter controls and wraps at either end. Shift+left/right continues to move between step banks.
- PageUp/PageDown moves to the previous/next step of the current track, wrapping at its length, while keeping the active parameter and BASE/LOCK scope. Shift+1 through Shift+6 jumps to the corresponding track; an incompatible parameter switches to that track's first compatible parameter.
- A number-row percentage assignment applies immediately, keeps the parameter editor open, and ramps the affected continuous control to its new value over approximately 30 ms like a quick fader movement.
- Up/down assignments apply immediately and keep the editor open for repeated changes. On Bass waveform, either direction switches between Saw and Square; on Chord chorus, Up/Down moves through Off, I, and II without wrapping.
- Enter or Esc returns to navigation without reverting changes already made.
- A series of repeated arrow changes to one value is coalesced into one undo transaction until the parameter changes, editing ends, or 300 ms elapses without another adjustment.
- Mute remains a discrete immediate action; Bass waveform and Chord chorus use discrete persistent editors.
- `C` opens a compact Chord-shape selector over the selected track's parameter section, keeping the sequencer visible. It reuses the LFO selector's vertical list: Up/Down selects a recipe and stops at the first or last value, PageUp/PageDown moves between steps and updates the selector to that step's shape, while Enter/Esc closes without reverting edits. On an existing Chord note only that note changes; on an empty step the track's future input shape changes. Ties display their source shape and must be edited at the source note.
- `Shift+L` on an eligible parameter immediately creates the default enabled sine, quarter-note, 10%-depth LFO when none exists, then opens its modal editor. Existing assignments open unchanged.
- The LFO modal uses left/right to select enabled, waveform, rate mode, rate, or depth; up/down adjusts the selected field, Shift+up/down changes percentage fields by 10, and number-row percentage entry applies to free rate and depth. Enter or Esc closes without reverting immediate edits. Backspace or Delete removes the assignment.
- `Shift+A` toggles accent immediately on a trigger or note. `Shift+G` toggles slide on a Bass note. Both are undoable and reject incompatible or empty steps visibly.

### 5.4 Audition behavior

Adding or replacing a trigger or note automatically auditions it while transport is stopped or paused. Clearing an event never auditions.

`o` explicitly auditions at any transport state without changing transport or pattern data:

- A drum row auditions the selected trigger's accent and locks, or an unaccented hit with base values on an empty step.
- A Bass/Lead note auditions its pitch, accent, and locks; a Chord note auditions its complete selected shape.
- A tie resolves and auditions its source note and accent while applying effective tie-chain locks.
- An empty pitched step auditions the last-entered note unaccented with base values.
- A synth audition holds the gate for one quarter note at the current tempo and then enters release.
- Explicit audition may overlap normal sequence playback through independent preview voices and chorus state without altering sequenced voice state.

## 6. Terminal interface

### 6.1 Layout

At `120x34` or larger, the normal screen contains:

1. Header: application name, project filename or `Untitled`, dirty marker, audio device/status, transport state, and tempo.
2. Global row: the six global controls, current values, and their local shortcuts.
3. Six variable-length sequencer track blocks: track name, mute state, absolute step range, and compact fixed-width step cells. The displayed range communicates the track length.
4. A selected-control panel: vertical parameter faders for the selected track, or six global detail cards when the global row is selected.
5. Status line: current mode, last successful operation or actionable error, and active-editor guidance. While a track parameter is being edited in `LOCK` scope, the selected-control panel and status line prominently show `LOCK PARAMETER EDITING` using contrasting styling.

Track percentage parameters use ten vertically stacked segments, filled proportionally and accompanied by an exact percentage. The selected-track title shows accent state, inherited accent/source on ties, and Bass slide state. Bass waveform and Chord chorus use the same column geometry as discrete switches. Mixer, instrument, filter, and envelope groups use distinct colors. The active parameter editor is marked with a heavy outline, reverse styling, and a bold label. In `LOCK` scope, faders show effective values and explicitly identify `LOCK` overrides versus `BASE`-inherited values. Physical units are shown in the active readout. A `~` badge marks parameters with an LFO assignment, including disabled assignments.

The compact, centered track-level LFO modal arranges enabled, waveform, rate mode, rate, and depth as five control columns from left to right, matching left/right field selection and up/down value adjustment. Its size is capped rather than expanding with larger terminals, and control names occupy their card borders to avoid duplicated labels and empty space. Enabled and rate mode use two-position switches, waveform and synchronized rate use multi-value selectors that fill all available rows, and free rate and depth use ten-segment faders. Up selects the displayed option above and Down selects the option below; both two-position switches and multi-value selectors stop at their first and last values instead of cycling. For faders, Up increases and Down decreases. The selected column uses the same heavy outline, reverse styling, and bold labeling as an active parameter. Rate shows its synchronized division or free percentage together with the resulting physical Hz value; depth is labeled in bipolar percentage points. The Chord-shape selector uses the same vertical-list treatment but is anchored over the selected track's parameter section so the sequencer remains visible while PageUp/PageDown changes the selected step.

Shortcuts are displayed beside the controls they operate: global keys in the global row/cards, event and navigation keys in the pattern title, track keys in the selected-track title, and parameter keys below their faders. The help overlay remains available for the complete key map; there is no persistent bottom instruction panel.

Step cells use these textual forms:

- `.` empty
- `x` / `X` normal/accented drum trigger
- `D:O` / `D!O` normal/accented note degree `D` at octave `O`
- `D*O` / `D#O` locked normal/accented note
- `-` tie
- `*` additional lock marker

Bass notes with slide are underlined.

The selected cell and currently playing cell have independent styling. If both refer to the same cell, the combined style must still communicate both states. Mute, event type, and lock state must not rely on color alone.

Each pitched row includes its current input octave in the track label (for example, `Bass O3`).

The sequencer grid uses 32 fixed-width cells per physical line with a visible divider after each 16-step bank. Steps 33 through 64 use a continuation line. Cells beyond a track's length are blank and cannot be selected. The detail panel is only as tall as its faders or global cards require, and all remaining vertical space is assigned to the sequencer. When expanded track blocks still exceed the pattern panel height, the panel scrolls by complete track blocks to keep the selected track visible. Wider terminals do not stretch individual step cells.

When the terminal is smaller than `120x34`, replace the main layout with the current size, required size, and quit/help keys. The project and audio engine remain active so resizing restores the normal view.

### 6.2 Modes and overlays

The current mode is always named on screen. Modes are:

- Navigation
- Parameter edit
- Tempo numeric input
- File-path input
- Unsaved-changes confirmation
- Error dialog
- Help

File prompts accept literal absolute paths or paths relative to the directory from which the process was launched. The MVP does not expand `~`, environment variables, or globs. The resolved path is shown before confirmation.

Open and quit with a dirty project present a `Save`, `Discard`, `Cancel` choice. Save failure leaves the confirmation open and never discards data. Opening a project stops and resets playback, clears effects, loads the new engine state, resets undo/redo history, selects the global row, and marks the project clean.

## 7. Undo, redo, and dirty state

- Retain up to 256 project-changing transactions in memory.
- Undoable changes include events, tie cleanup, locks, base parameters, global parameters, waveform, mute, input degree/octave, and project-wide edits.
- Transport, cursor position, selected section, active mode, status messages, and audition are not undoable.
- A new edit after undo clears redo history.
- Loading a project or creating a new project clears both histories.
- Undo history is never serialized.
- Dirty state is based on whether the current project model equals the last successfully loaded or saved revision. Undoing back to that revision clears the dirty marker; redoing away from it restores the marker.
- An edit must not be committed to the UI model unless its corresponding engine command can be queued. A full engine queue produces a visible error instead of allowing UI/audio state to diverge.

## 8. Project file format

### 8.1 General rules

- Project files are UTF-8, pretty-printed JSON ending with a newline.
- The conventional extension is `.groove.json`, but the application does not silently alter a user-supplied filename.
- Version 7 is strict: reject unknown fields, enum values, invalid numeric ranges, incorrect track layouts, pattern counts other than 100, step counts outside 1 through 64, incompatible events/locks/LFOs, and invalid tie graphs. Version 6 is migrated; versions 1 through 5 and unknown future versions are rejected.
- A failed load leaves the current project, undo history, dirty state, and engine untouched.
- A successful save writes a temporary sibling file, flushes it, and atomically renames it over the destination. A failed save leaves the previous destination intact and the current project dirty.

### 8.2 Logical schema

The top-level object is:

```json
{
  "format_version": 7,
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
  "reverb_tone": 50,
  "reverb_pre_delay_ms": 20,
  "key": "C",
  "scale": "major"
}
```

`delay_division` accepts the stable strings `thirty_second`, `sixteenth_triplet`, `sixteenth`, `eighth_triplet`, `eighth`, `eighth_dotted`, `quarter_triplet`, `quarter`, `quarter_dotted`, `half`, and `bar`.

`tracks` contains exactly six entries in the fixed instrument order. Every track stores:

- `kind`: `kick`, `snare`, `hat`, `bass`, `chord`, or `lead`
- `name`: the fixed display identifier
- `level`: integer 0–100
- `muted`: Boolean
- `delay_send`: integer 0–100
- `reverb_send`: integer 0–100
- An `instrument` object with the applicable base values
- A required sparse `lfos` object containing compatible per-destination assignments
- A `steps` array containing 1 through 64 elements; its array length is the track length
- Bass, Chord, and Lead additionally store `input_degree` and `input_octave`. Chord tracks may store `input_chord_shape`; omitted values mean `1-3-5`.

Chord instruments store `oscillator_mix`, `pulse_width`, `sub_oscillator`, `chorus`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `decay`, `sustain`, and `release`. Lead stores the same percentage controls except `chorus`. Bass retains `waveform`, `cutoff`, `resonance`, `filter_envelope`, and `decay`.

An empty step is JSON `null`. Populated step shapes are:

```json
{
  "type": "trigger",
  "accent": true,
  "locks": {
    "tune": 70
  }
}
```

```json
{
  "type": "bass_note",
  "degree": 5,
  "octave": 3,
  "accent": false,
  "slide": true,
  "locks": {
    "cutoff": 40,
    "waveform": "square"
  }
}
```

`accent` is required and Boolean on triggers, `bass_note`, and `note`. `slide` is additionally required and Boolean on `bass_note`. Both are invalid on ties.

Chord notes may include an optional `chord_shape` string. Omitted values mean `triad_root` (`1-3-5`). The stable shape names are `triad_root`, `triad_first_inversion`, `triad_second_inversion`, `seventh_root`, `seventh_first_inversion`, `seventh_second_inversion`, `seventh_third_inversion`, `sixth_root`, `sixth_first_inversion`, `sixth_second_inversion`, `sixth_third_inversion`, `sus2_root`, `sus2_first_inversion`, `sus2_second_inversion`, `sus4_root`, `sus4_first_inversion`, and `sus4_second_inversion`. The field is invalid on Lead notes.

```json
{
  "type": "tie",
  "locks": {
    "filter_envelope": 80
  }
}
```

The `locks` object is always present on populated steps and contains only overridden values. Lock keys use the stable names `level`, `delay_send`, `reverb_send`, `tune`, `tone`, `snappy`, `decay`, `waveform`, `oscillator_mix`, `pulse_width`, `sub_oscillator`, `chorus`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `sustain`, and `release`, subject to track compatibility. Chord chorus values are `off`, `i`, and `ii`. `mute`, `accent`, and `slide` are invalid in a lock object.

An empty LFO collection is `{}`. Assignment keys are the compatible continuous instrument parameters plus `level`; Bass waveform, Chord chorus, and mixer sends are excluded. A synchronized assignment is:

```json
"lfos": {
  "cutoff": {
    "enabled": true,
    "waveform": "sine",
    "rate": { "mode": "synced", "division": "quarter" },
    "depth": 10
  }
}
```

A free rate uses `{ "mode": "free", "rate_percent": 50 }`. Waveform names are `sine`, `triangle`, `square`, `saw`, and `sample_and_hold`. Synchronized division names are `four_bars`, `two_bars`, `bar`, `half`, `quarter_dotted`, `quarter`, `quarter_triplet`, `eighth_dotted`, `eighth`, `eighth_triplet`, `sixteenth`, `sixteenth_triplet`, and `thirty_second`.

### 8.3 Model types

The Rust model should express and validate the schema through these domain concepts rather than untyped maps:

- `ProjectV6`
- `Globals`
- `Track` with fixed `TrackKind`
- `KickParameters`, `SnareParameters`, `HatParameters`, `BassParameters`, `ChordParameters`, and `LeadParameters`
- `Step`
- `StepEvent::{Trigger, BassNote, Note, Tie}`
- `ChordShape`
- `ParameterLocks`
- `ParameterId`
- `LfoAssignments`, `LfoConfig`, `LfoWaveform`, `LfoRate`, and `LfoDivision`
- bounded `Percent`
- `PitchClass`
- `Scale::{Major, NaturalMinor}`
- `Waveform::{Square, Saw}`
- `ChorusMode::{Off, I, Ii}`
- `DelayDivision`

There is no stable public Rust library API in the MVP. The public compatibility interfaces are the CLI, keyboard behavior, and JSON schema.

## 9. Command-line interface

```text
terminal-groove [PROJECT] [--audio-device <exact-name>]
terminal-groove --list-audio-devices
terminal-groove --help
terminal-groove --version
```

- With no project argument, start a new untitled project using all defaults.
- With a project argument, validate the entire project before entering the TUI.
- The default output device is used unless `--audio-device` is given.
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

Ratatui documents backend compatibility and provides a test backend, CPAL documents the Linux ALSA requirement, and `rtrb` provides a wait-free fixed-capacity SPSC queue:

- <https://ratatui.rs/concepts/backends/>
- <https://github.com/RustAudio/cpal>
- <https://docs.rs/crate/rtrb/latest>

Linux setup documentation must include Rust installation and:

- Debian/Ubuntu: `libasound2-dev`
- Fedora: `alsa-lib-devel`

The current development workspace does not yet have Rust or the ALSA development package installed.

### 10.2 Package organization

Use one binary package with testable library modules grouped by behavior:

- Model and validation
- Reducer, command handling, undo/redo
- Project serialization and atomic file I/O
- TUI rendering and terminal lifecycle
- Sequencer and transport
- DSP primitives, instruments, effects, and offline renderer
- CPAL host/stream integration

Keep the model/reducer and DSP independent from Ratatui and CPAL so they can be tested without a terminal or audio device.

### 10.3 Threading and real-time rules

- The main thread owns terminal input, rendering, dialogs, undo/redo, file I/O, and the canonical editable project.
- CPAL's audio callback owns transport timing, a mirrored engine project, voices, filters, effects, and sample conversion.
- UI-to-audio communication uses a preallocated bounded SPSC queue containing fixed-size typed mutations and transport/audition commands.
- Independent per-track playheads and transport telemetry return through atomics or a second bounded channel where intermediate redundant playhead updates may be dropped.
- Project load happens while transport is stopped and may rebuild the audio engine outside the callback.
- The callback must not allocate, free heap-backed messages, lock a mutex, block, access the filesystem, format text, or log.
- Noise generators use preallocated deterministic PRNG state local to each voice.
- Queue exhaustion is handled on the UI side before committing the model change.
- CPAL stream errors are forwarded to the UI through a non-blocking error path and shown prominently.

### 10.4 Audio format and scheduling

- Use the selected device's default output configuration and sample rate.
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
- DSP must replace any unexpected non-finite intermediate or sample with zero before it reaches the device and surface a diagnostic outside the callback.
- Terminal cleanup uses RAII and a panic hook so the shell is not left in raw mode.

## 12. Testing and acceptance

### 12.1 Unit tests

- Major and natural-minor degree mapping in all 12 keys
- Degree 8 octave behavior and input-octave limits
- Frequency conversion with A4 = 440 Hz
- Tie creation, wrapped resolution, all-tie rejection, and dependent-tie cleanup
- Bass/Lead gate transitions and Chord shape generation, eight-voice overlap, retrigger, tie, and release behavior
- Trigger/note accent defaults, tie inheritance, engine-specific timbre response, and latched tail behavior
- Bass slide arming through ties, fixed 60 ms glide time, and legato envelope behavior
- One-step lock overlay and restoration
- Lock compatibility by track and event type
- Number-row percentage mapping and arrow clamping
- Undo/redo ordering, coalescing, redo invalidation, and dirty-revision restoration
- Tempo step-length accumulation without cumulative drift
- Independent track cycles, live resizing, and reset/resume positions
- Resizing and exact sequence doubling at valid boundaries, including undo and rejection above 32 steps
- Delay-division duration at representative tempos and sample rates
- LFO destination compatibility, free/synchronized rates, phase reset/freeze, and lock-centered clamping

### 12.2 Persistence tests

- Golden JSON for every track/event/lock and LFO rate/waveform variant
- Default-project and populated-project round trips
- Rejection of unknown versions/fields, bad ranges, wrong track order/count, step counts outside 1–64, invalid locks/LFO assignments, and invalid ties
- Strict version 6 round trips, defaulting omitted Chord-shape fields to `1-3-5`, and rejection of versions 1 through 5 without migration
- Failed loads preserve the active project
- Atomic saves leave the previous file intact on failure
- Successful save/load resets dirty state and load resets history

### 12.3 TUI and reducer tests

- Ratatui `TestBackend` rendering at `120x34` and larger
- Fixed-width 32-cell rows, 16-step bank divider, continuation rows, and track-block scrolling
- Physical-row vertical navigation, bank navigation, length input, and doubling shortcut
- Small-terminal resize screen
- Ten-segment fader fill, waveform/chorus switches, active-parameter styling, and local shortcut labels
- Effective `LOCK` values with explicit/inherited origin labels
- Global detail cards and physical active-parameter readouts
- Independent playhead and cursor styling
- Non-color event and lock indicators
- Every documented shortcut in its valid and invalid contexts
- BASE/LOCK scope persistence and reset rules
- Parameter-mode precedence over normal up/down navigation
- Accent and Bass-slide editing, source-note articulation readout on ties, and BASE/LOCK independence
- LFO modal navigation, immediate edits, assignment badges, removal, and minimum-size rendering
- Chord-shape selector navigation, per-note/default editing, inversion display, and minimum-size rendering
- File prompts, dirty confirmations, error dialogs, and help overlay
- Terminal restoration on normal and simulated failure paths

### 12.4 DSP tests

- Oscillator pitch and alias-reduced bounded output
- ADSR stage durations within tolerance at multiple sample rates
- Stable filter output at all cutoff/resonance extremes
- Finite drum output and expected decay ordering at 0%, 50%, and 100%
- Step-lock changes and smoothing without discontinuity spikes
- Bounded LFO waveforms, deterministic sample-and-hold, synchronized/free rate accuracy, transport phase behavior, and discontinuity smoothing
- Delay timing, feedback decay, maximum-delay allocation, and click-free time changes
- Reverb decay-time monotonicity and bounded output
- Limiter ceiling, fixed makeup gain, representative-groove RMS, and sample-format conversion bounds
- Deterministic offline rendering from a fixed project and PRNG seed
- A callback-path allocation test or equivalent instrumentation proving no real-time heap activity

### 12.5 Manual acceptance scenarios

1. Start an untitled project in a `120x34` terminal, move to each row, enter events, and see all state and local shortcuts without opening help.
2. Build and hear a drum loop using Enter; edit the Kick tune/decay/attack, Snare tune/tone/snappy, and Hi-hat tune/decay; add accents; mute tracks; and use both effect sends.
3. Enter Bass degrees and octave changes, toggle accent and slide, create ordinary and loop-wrapped ties, and hear the fixed-time 303-style glide, mono envelope, and inherited-accent behavior.
4. Add base values and locks while stopped and playing; verify locks apply only on their step and live edits take effect on the next pass.
5. Edit each track parameter and confirm its fader fills, shortcut, active highlight, exact percentage, and physical readout. Add synced and free LFOs, verify their badges and modal values, and hear locks remain the modulation center.
6. Audition empty and occupied drum/pitched steps with `o`, including Chord shapes and inversions while transport is running, without pattern changes.
7. Change key and scale and verify existing degree data follows the new harmony on future triggers.
8. Undo and redo compound tie cleanup and repeated parameter edits, including returning to the saved clean revision.
9. Save, inspect, reopen, and compare a version 6 project with accents, Bass slides, Chord shapes/inversions, Chord/Lead settings, ties, locks, effects, mute states, and input octaves; verify versions 1 through 5 are rejected without altering the active project.
10. List audio devices, use the default device, and select a unique explicit device.
11. Play for at least ten minutes at a supported 48 kHz low-latency configuration without stream errors, non-finite output, timing drift, or audible clicks from normal parameter edits.
12. Exit normally and simulate startup/runtime failures, confirming that the terminal is always restored.

## 13. MVP completion criteria

The MVP is complete when all automated tests pass and every manual acceptance scenario succeeds on a current Linux desktop using ALSA directly or through the system's configured sound-server bridge. The implementation must match the documented keyboard map and JSON schema, generate all sound procedurally, keep audio scheduling independent from TUI redraw timing, and leave no hidden editing mode or silent data-loss path.
