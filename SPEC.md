# terminal-groove MVP specification

Status: implementation-ready MVP specification  
Target: Linux-first terminal application  
Application and executable name: `terminal-groove`

## 1. Product definition

`terminal-groove` is a real-time groovebox operated entirely from a terminal. Its primary design goals are a fast keyboard workflow, predictable state transitions, and a transparent interface: the selected section and step, transport state, active editing mode, parameter values, triggers, notes, ties, locks, available shortcuts, dirty state, and audio errors must remain visible.

The MVP has one continuously looping, one-bar pattern. The bar is fixed to 4/4 and divided into 16 sixteenth-note steps shared by all tracks.

There are exactly six tracks in this fixed order:

1. Kick drum
2. Snare drum
3. Hi-hat
4. Mono synth 1
5. Mono synth 2
6. Mono synth 3

All sound is synthesized in real time. The application contains no audio samples.

### 1.1 MVP capabilities

- Start, pause, resume, stop, and reset pattern playback.
- Edit the pattern while it is playing. An edit affects playback the next time the affected step is reached.
- Add drum triggers, synth notes, synth ties, and per-step parameter locks.
- Set per-track level, mute, delay send, and reverb send.
- Configure tempo, delay time and feedback, reverb time, musical key, and major or natural-minor scale.
- Audition sounds without modifying the pattern.
- Undo and redo project edits within the current session.
- Save and load versioned, human-readable JSON projects.
- Select the system default audio output or an explicit output device from the command line.

### 1.2 Explicitly excluded from the MVP

- Samples, sample import, or sample playback
- MIDI input, output, or clock sync
- WAV or other audio export
- Multiple patterns, pattern chaining, or song mode
- Per-track pattern lengths, polymeter, or time-signature changes
- Swing, microtiming, probability, velocity, or accents
- Polyphonic synth tracks, portamento, or oscillator detune
- Per-track pan, solo, master-volume control, or configurable effect returns
- User-defined track types or track reordering
- Mouse control
- Plug-ins or external effects

## 2. Sequencer model

### 2.1 Timing and transport

- Every track has exactly 16 steps, with one step equal to one sixteenth note.
- At tempo `BPM`, the nominal duration of one step is `sample_rate * 60 / (BPM * 4)` samples.
- The audio engine must use fractional sample accumulation rather than rounding every step independently, preventing cumulative timing drift.
- Starting from the reset state triggers step 1 immediately.
- `Space` while playing pauses before the next unplayed step. Active synth gates are released, while delay and reverb tails continue.
- `Space` while paused resumes by triggering the next unplayed step immediately and establishing a new timing origin.
- `.` stops playback, resets the next step to step 1, releases all voices, and clears delay and reverb state.
- The edit cursor is independent from the playhead.
- Tempo changes become active at the next step boundary without skipping or repeating a step.

### 2.2 Drum events

A drum step is either empty or contains one trigger. Adding a trigger to an empty step creates it; pressing `Enter` on an occupied drum step clears the trigger and all of its locks.

Each drum track is a single retriggerable synthesized voice. A new trigger restarts that voice rather than creating overlapping polyphony. Instrument parameters captured when a trigger starts, such as tone and decay, remain part of that hit's state. Mixer level and send values continue to follow the effective value of each sequencer step.

### 2.3 Synth events

A synth step is exactly one of:

- Empty
- A note containing a scale degree from 1 through 8 and an input octave from 0 through 7
- A tie

Degree 1 is the root in the stored input octave. Degrees 2 through 7 use the selected scale, and degree 8 is the root one octave above degree 1. Pitch uses twelve-tone equal temperament with A4 = 440 Hz.

Notes are stored as scale degree and octave rather than absolute pitch. Changing the global key or scale therefore reinterprets all existing pattern notes the next time they trigger. A note that is already sounding keeps its current pitch until a new note event starts; a tie does not retune it.

Each synth track is strictly monophonic:

- A note event sets pitch, opens the gate, and retriggers the amplitude envelope.
- A following tie keeps the gate open without retriggering pitch or the envelope.
- A following empty step closes the gate and begins release.
- A following note closes/restarts the existing voice at the new pitch, with click-safe envelope handling.
- There is no glide between notes.

Pressing a degree key replaces any existing event on the selected step with that note but preserves compatible parameter locks. Pressing `Enter` on an empty synth step inserts the track's last-entered degree and input octave; the initial last-entered degree is 1. Pressing `Enter` on a note or tie clears the event and its locks.

### 2.4 Tie invariants

- A tie is valid only when its immediately preceding step, considered cyclically, is a note or another valid tie that resolves to a note.
- Step 1 may tie from step 16.
- A pattern containing only ties is invalid.
- Adding a tie to an empty step requires a valid predecessor. Otherwise, the edit is rejected and a visible status message explains why.
- Pressing `t` on a tie clears it and its locks.
- Pressing `t` on a note replaces the note with a tie only if the resulting tie graph remains valid.
- Clearing or replacing a source note also clears the contiguous following ties that would become invalid, including a wrapped chain. This is recorded as one undoable operation.
- When playback or audition begins on a tie without an already-active voice, the engine resolves the source note cyclically and retriggers it at that boundary to establish the held voice. Continuous playback across the loop does not retrigger a valid wrapped tie.

### 2.5 Parameter locks

Every track has base parameter values. A step may contain a sparse set of parameter locks that overlay those values for that step only.

- Locks are permitted only on a drum trigger, synth note, or synth tie.
- Instrument parameters, waveform, level, delay send, and reverb send are lockable.
- Mute is never lockable.
- At each boundary, the engine computes effective values by overlaying the current step's locks on the base values.
- At the next boundary, an unlocked parameter returns to its base value or takes the next step's lock.
- Continuous changes are smoothed to prevent clicks.
- A drum tone or decay lock initializes the triggered drum voice and therefore remains audible for that hit's tail. Step-level mixer locks still expire at the next boundary.
- Synth locks on a tie can update waveform, filter, envelope settings, level, and sends without retriggering the note. Changing attack during a tie does not restart the attack phase.
- Clearing an event also clears every lock on that step.

The UI has a persistent parameter scope with two visibly labelled states: `BASE` and `LOCK`. `p` toggles the scope on a track. The scope persists while moving between steps on that track and resets to `BASE` when the user changes tracks, selects the global row, or presses `Esc` from navigation mode.

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

### 3.2 Kick drum

The kick is generated by a continuously running sine oscillator with independent pitch and amplitude envelopes. Retriggering does not reset oscillator phase; both envelopes continue smoothly from their instantaneous values to avoid clicks.

- `tone`: maps the pitch-envelope peak linearly from 75 Hz to 220 Hz and the settled fundamental linearly from 38 Hz to 58 Hz. On a trigger, pitch ramps linearly to the peak over approximately 1.5 ms and then exponentially to the settled frequency over at most 130 ms.
- `decay`: maps linearly from approximately 80 ms to 830 ms.

The amplitude ramps exponentially to a peak of 1.2 over approximately 4 ms, then exponentially to an inaudible floor at the selected decay time.

Defaults: tone 50%, decay 35%.

### 3.3 Snare drum

The snare combines one continuously running triangle body oscillator with band-pass-filtered pseudorandom white noise. No stored noise sample is permitted, and retriggering does not reset oscillator phase or filter state.

- `tone`: linearly raises the triangle frequency from 145 Hz to 315 Hz, the noise band-pass center from 800 Hz to 6 kHz, and the filter Q from 0.6 to 5.6. The noise and body gains are fixed at 0.75 and 0.35 respectively.
- `decay`: maps linearly from approximately 50 ms to 550 ms.

The shared amplitude envelope ramps exponentially to a peak of 0.85 over approximately 1 ms, then exponentially to an inaudible floor at the selected decay time.

Defaults: tone 50%, decay 35%.

### 3.4 Hi-hat

The hi-hat uses continuously generated pseudorandom white noise through a resonant high-pass filter. No stored noise sample is permitted, and retriggering does not reset filter state.

- `tone`: linearly raises the high-pass cutoff from 2.8 kHz to 11.8 kHz, clamped below Nyquist. Filter Q is fixed at 1.2.
- `decay`: maps linearly from approximately 25 ms to 345 ms.

The amplitude envelope ramps exponentially to a peak of 0.55 over approximately 1 ms, then exponentially to an inaudible floor at the selected decay time.

Defaults: tone 60%, decay 20%.

### 3.5 Mono synths

Each synth contains:

- One independently selectable band-limited square or saw oscillator
- A stable two-pole topology-preserving-transform state-variable low-pass filter
- One ADSR amplitude envelope
- Positive filter modulation driven by that same ADSR

Parameters:

- `waveform`: square or saw; default saw.
- `cutoff`: exponential mapping from 20 Hz to the lower of 20 kHz or 45% of sample rate; default 65%.
- `resonance`: approximately Q 0.707 through Q 10 while remaining stable at all cutoff values; default 10%.
- `filter envelope`: 0% adds no modulation and 100% adds up to six octaves to the base cutoff, clamped to the safe cutoff limit; default 25%.
- `attack`: 0% is effectively instantaneous; 1–100% map exponentially from about 1 ms to 2 seconds; default 0%.
- `decay`: exponential mapping from 5 ms to 3 seconds; default 25%.
- `sustain`: linear amplitude from 0% to 100%; default 70%.
- `release`: exponential mapping from 5 ms to 5 seconds; default 15%.

All synths default to input degree 1 and octave 3.

### 3.6 Mixer and effects

Each track provides:

- Level, default 80%
- Mute, default off
- Delay send, default 0%
- Reverb send, default 0%

Sends are post-fader and post-mute. Muting ramps the dry track and new send input to silence, but already-generated global effect tails continue. A muted synth voice continues its internal state, so unmuting may reveal a still-active voice.

The internal engine renders stereo. A mono output device receives an equal-power mono sum. On devices with more than two channels, channels 1 and 2 receive left and right and additional channels receive silence.

#### Delay

The delay is a tempo-synchronized stereo cross-feedback delay with no independent return-level control. Supported divisions, in selection order, are:

`1/32`, `1/16T`, `1/16`, `1/8T`, `1/8`, `1/8D`, `1/4T`, `1/4`, `1/4D`, `1/2`, and `1 bar`.

Triplet values are two-thirds of their straight counterpart; dotted values are one-and-a-half times their straight counterpart. At the minimum tempo, the implementation must preallocate enough delay memory for the longest supported value. Delay-time or tempo changes crossfade between taps rather than abruptly changing the read position.

Feedback ranges from 0% through 95% to prevent unity or unstable feedback. Default division is `1/8`; default feedback is 30%.

#### Reverb

The reverb is a stereo algorithmic Schroeder/Freeverb-style network using parallel feedback comb filters followed by series all-pass filters. It has no samples, convolution impulse, pre-delay control, damping control, or independent return-level control in the MVP.

Reverb time ranges from 0.2 through 10.0 seconds and defaults to 2.5 seconds.

#### Master safety

The final output stage applies DC blocking and a transparent soft safety limiter. It must prevent non-finite values and keep converted samples inside the device format's valid range. The limiter is not exposed as a user parameter.

## 4. Global musical parameters

| Parameter | Range | Default | Editing behavior |
| --- | --- | --- | --- |
| Tempo | Integer 40–240 BPM | 120 BPM | Type a complete BPM value and press Enter, or use up/down by 1 and Shift+up/down by 5 |
| Delay time | Supported division list | `1/8` | Up/down moves through the list |
| Delay feedback | 0–95% | 30% | Percentage direct entry and arrows |
| Reverb time | 0.2–10.0 s | 2.5 s | Up/down by 0.1 s and Shift+up/down by 1 s |
| Key | C, C#, D, D#, E, F, F#, G, G#, A, A#, B | C | Up/down moves chromatically |
| Scale | Major, natural minor | Major | Up/down or the shortcut toggles the value |

Enharmonic keys use sharp names in the MVP. Major uses semitone offsets `[0, 2, 4, 5, 7, 9, 11, 12]`; natural minor uses `[0, 2, 3, 5, 7, 8, 10, 12]`.

Global parameters cannot be parameter-locked.

## 5. Keyboard interaction

The application uses ordinary portable terminal press events. It must not require Kitty keyboard-protocol release events. Mouse capture is not enabled.

### 5.1 Navigation mode

- Up/down selects the global row or one of the six track rows. Vertical navigation clamps at the first and last row.
- On a track, left/right moves the shared step cursor and wraps between steps 1 and 16.
- On the global row, left/right cycles through global parameters and wraps.
- Returning from the global row to a track restores the shared step cursor.
- `Enter` edits the selected global control or toggles/inserts the selected track event as defined in the sequencer model.
- `Backspace` or `Delete` clears the selected event and its locks.
- `Esc` exits overlays or parameter editing first; from track navigation it returns lock scope to `BASE`.

### 5.2 Key map

| Context | Key | Action |
| --- | --- | --- |
| Anywhere | `Space` | Play/pause |
| Anywhere | `.` | Stop, reset, and clear effect tails |
| Anywhere | `o` | Audition selected track/step without editing |
| Anywhere | `Ctrl+S` | Save, prompting if no current path exists |
| Anywhere | `Ctrl+Shift+S` | Save as |
| Anywhere | `Ctrl+O` | Open project |
| Anywhere | `Ctrl+Q` | Quit, with dirty confirmation |
| Anywhere | `Ctrl+Z` | Undo |
| Anywhere | `Ctrl+Y` | Redo |
| Anywhere | `?` | Toggle full help overlay |
| Track | `p` | Toggle visible `BASE`/`LOCK` scope |
| Track | `v` | Edit level |
| Track | `m` | Toggle mute immediately |
| Track | `y` | Edit delay send |
| Track | `b` | Edit reverb send |
| Drum | `t` | Edit tone |
| Drum | `d` | Edit decay |
| Synth | `1`–`8` | Insert/replace note at current input octave |
| Synth | `[` / `]` | Decrease/increase input octave, clamped to 0–7 |
| Synth | `t` | Insert, replace with, or clear a tie subject to validation |
| Synth | `w` | Toggle square/saw in the active parameter scope |
| Synth | `c` | Edit cutoff |
| Synth | `R` | Edit resonance |
| Synth | `f` | Edit filter-envelope amount |
| Synth | `a` | Edit attack |
| Synth | `d` | Edit decay |
| Synth | `s` | Edit sustain |
| Synth | `r` | Edit release |
| Global | `t` | Edit tempo |
| Global | `y` | Edit delay division |
| Global | `f` | Edit delay feedback |
| Global | `r` | Edit reverb time |
| Global | `k` | Edit musical key |
| Global | `s` | Toggle/edit scale |

Shortcuts are resolved by selected section, so repeated letters do not conflict.

### 5.3 Parameter editing mode

- Pressing a parameter shortcut enters a visibly labelled value editor.
- Pressing another valid parameter shortcut switches the editor to that parameter without leaving the current BASE/LOCK scope.
- A number-row percentage assignment applies immediately and returns to navigation mode.
- Arrow assignments apply immediately and keep the editor open for repeated changes.
- Enter or Esc returns to navigation without reverting changes already made.
- A series of repeated arrow changes to one value is coalesced into one undo transaction until the parameter changes, editing ends, or 300 ms elapses without another adjustment.
- Waveform and mute are discrete immediate actions rather than percentage editors.

### 5.4 Audition behavior

Adding or replacing a trigger or note automatically auditions it only while transport is stopped. Clearing an event never auditions.

`o` explicitly auditions at any transport state without changing transport or pattern data:

- A drum row always auditions that drum, applying the selected step's locks if it contains a trigger.
- A synth note auditions that note and its locks.
- A synth tie resolves and auditions its source note while applying the tie's locks.
- An empty synth step auditions the last-entered note using base values.
- A synth audition holds the gate for one quarter note at the current tempo and then enters release.
- Explicit audition may overlap normal sequence playback as a temporary preview voice, but must not alter the sequenced mono voice's persistent state.

## 6. Terminal interface

### 6.1 Layout

At `80x24` or larger, the normal screen contains:

1. Header: application name, project filename or `Untitled`, dirty marker, audio device/status, transport state, and tempo.
2. Global row: the six global controls and their current values.
3. Six fixed sequencer rows: track name, mute state, and 16 step cells.
4. Parameter detail panel: selected track/global values, physical units, BASE/LOCK scope, and selected lock inheritance.
5. Status line: last successful operation or actionable error.
6. Persistent contextual shortcut legend.

Step cells use these textual forms:

- `.` empty
- `x` drum trigger
- `1`–`8` synth note degree
- `-` tie
- `*` additional lock marker

The selected cell and currently playing cell have independent styling. If both refer to the same cell, the combined style must still communicate both states. Mute, event type, and lock state must not rely on color alone.

When the terminal is smaller than `80x24`, replace the main layout with the current size, required size, and quit/help keys. The project and audio engine remain active so resizing restores the normal view.

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
- Version 1 is strict: reject unknown versions, fields, enum values, invalid numeric ranges, incorrect track layouts, incorrect step counts, incompatible lock names, and invalid tie graphs.
- A failed load leaves the current project, undo history, dirty state, and engine untouched.
- A successful save writes a temporary sibling file, flushes it, and atomically renames it over the destination. A failed save leaves the previous destination intact and the current project dirty.

### 8.2 Logical schema

The top-level object is:

```json
{
  "format_version": 1,
  "globals": {},
  "tracks": []
}
```

`globals` contains:

```json
{
  "tempo_bpm": 120,
  "delay_division": "eighth",
  "delay_feedback": 30,
  "reverb_time_seconds": 2.5,
  "key": "C",
  "scale": "major"
}
```

`delay_division` accepts the stable strings `thirty_second`, `sixteenth_triplet`, `sixteenth`, `eighth_triplet`, `eighth`, `eighth_dotted`, `quarter_triplet`, `quarter`, `quarter_dotted`, `half`, and `bar`.

`tracks` contains exactly six entries in the fixed instrument order. Every track stores:

- `kind`: `kick`, `snare`, `hat`, or `synth`
- `name`: the fixed display identifier
- `level`: integer 0–100
- `muted`: Boolean
- `delay_send`: integer 0–100
- `reverb_send`: integer 0–100
- An `instrument` object with the applicable base values
- A 16-element `steps` array
- Synths additionally store `input_degree` and `input_octave`

An empty step is JSON `null`. Populated step shapes are:

```json
{
  "type": "trigger",
  "locks": {
    "tone": 70
  }
}
```

```json
{
  "type": "note",
  "degree": 5,
  "octave": 3,
  "locks": {
    "cutoff": 40,
    "waveform": "square"
  }
}
```

```json
{
  "type": "tie",
  "locks": {
    "filter_envelope": 80
  }
}
```

The `locks` object is always present on populated steps and contains only overridden values. Lock keys use the stable names `level`, `delay_send`, `reverb_send`, `tone`, `decay`, `waveform`, `cutoff`, `resonance`, `filter_envelope`, `attack`, `sustain`, and `release`, subject to track compatibility. `mute` is invalid in a lock object.

### 8.3 Model types

The Rust model should express and validate the schema through these domain concepts rather than untyped maps:

- `ProjectV1`
- `Globals`
- `Track` with fixed `TrackKind`
- `DrumParameters` and `SynthParameters`
- `Step`
- `StepEvent::{DrumTrigger, SynthNote, Tie}`
- `ParameterLocks`
- `ParameterId`
- bounded `Percent`
- `PitchClass`
- `Scale::{Major, NaturalMinor}`
- `Waveform::{Square, Saw}`
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
- Playhead and transport telemetry return through atomics or a second bounded channel where intermediate redundant playhead updates may be dropped.
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
- Synth gate/retrigger/release transitions
- One-step lock overlay and restoration
- Lock compatibility by track and event type
- Number-row percentage mapping and arrow clamping
- Undo/redo ordering, coalescing, redo invalidation, and dirty-revision restoration
- Tempo step-length accumulation without cumulative drift
- Delay-division duration at representative tempos and sample rates

### 12.2 Persistence tests

- Golden JSON for every track/event/lock variant
- Default-project and populated-project round trips
- Rejection of unknown versions/fields, bad ranges, wrong track order/count, wrong step count, invalid locks, and invalid ties
- Failed loads preserve the active project
- Atomic saves leave the previous file intact on failure
- Successful save/load resets dirty state and load resets history

### 12.3 TUI and reducer tests

- Ratatui `TestBackend` rendering at `80x24` and larger
- Small-terminal resize screen
- Independent playhead and cursor styling
- Non-color event and lock indicators
- Every documented shortcut in its valid and invalid contexts
- BASE/LOCK scope persistence and reset rules
- Parameter-mode precedence over normal up/down navigation
- File prompts, dirty confirmations, error dialogs, and help overlay
- Terminal restoration on normal and simulated failure paths

### 12.4 DSP tests

- Oscillator pitch and alias-reduced bounded output
- ADSR stage durations within tolerance at multiple sample rates
- Stable filter output at all cutoff/resonance extremes
- Finite drum output and expected decay ordering at 0%, 50%, and 100%
- Step-lock changes and smoothing without discontinuity spikes
- Delay timing, feedback decay, maximum-delay allocation, and click-free time changes
- Reverb decay-time monotonicity and bounded output
- Limiter and sample-format conversion bounds
- Deterministic offline rendering from a fixed project and PRNG seed
- A callback-path allocation test or equivalent instrumentation proving no real-time heap activity

### 12.5 Manual acceptance scenarios

1. Start an untitled project in an `80x24` terminal, move to each row, enter events, and see all state and shortcuts without opening help.
2. Build and hear a drum loop using Enter, edit tone/decay, mute tracks, and use both effect sends.
3. Enter synth degrees and octave changes, create ordinary and loop-wrapped ties, and hear correct mono envelope behavior.
4. Add base values and locks while stopped and playing; verify locks apply only on their step and live edits take effect on the next pass.
5. Audition empty and occupied drum/synth steps with `o`, including while transport is running, without pattern changes.
6. Change key and scale and verify existing degree data follows the new harmony on future triggers.
7. Undo and redo compound tie cleanup and repeated parameter edits, including returning to the saved clean revision.
8. Save, inspect, reopen, and compare a project with notes, ties, locks, effects, mute states, and input octaves.
9. List audio devices, use the default device, and select a unique explicit device.
10. Play for at least ten minutes at a supported 48 kHz low-latency configuration without stream errors, non-finite output, timing drift, or audible clicks from normal parameter edits.
11. Exit normally and simulate startup/runtime failures, confirming that the terminal is always restored.

## 13. MVP completion criteria

The MVP is complete when all automated tests pass and every manual acceptance scenario succeeds on a current Linux desktop using ALSA directly or through the system's configured sound-server bridge. The implementation must match the documented keyboard map and JSON schema, generate all sound procedurally, keep audio scheduling independent from TUI redraw timing, and leave no hidden editing mode or silent data-loss path.
