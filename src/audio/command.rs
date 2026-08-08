use super::voices::{ArpeggioState, DRUM_SILENCE};
use super::{AudioProject, ParameterSmoothing, PatternIndexMap, Renderer, TRACK_COUNT};
use std::sync::atomic::Ordering;

#[derive(Clone, Debug)]
pub enum AudioCommand {
    PlayPause,
    Stop,
    SelectPattern {
        pattern: u8,
    },
    ReplaceProject {
        project: Box<AudioProject>,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    },
    Audition {
        track: u8,
        step: u8,
    },
    AutoAudition {
        track: u8,
        step: u8,
    },
}

pub(super) fn handle(renderer: &mut Renderer, command: AudioCommand) {
    match command {
        AudioCommand::PlayPause => {
            if !renderer.playing && renderer.status.paused.load(Ordering::Acquire) {
                renderer.clock.restart_timing();
            }
            renderer.playing = !renderer.playing;
            renderer
                .status
                .running
                .store(renderer.playing, Ordering::Release);
            renderer
                .status
                .paused
                .store(!renderer.playing, Ordering::Release);
            if !renderer.playing {
                for v in &mut renderer.synth {
                    v.gate_off();
                    v.active = false;
                }
                Renderer::release_chord(&mut renderer.chord);
                renderer.chord.arpeggio = ArpeggioState::default();
            }
        }
        AudioCommand::Stop => {
            renderer.playing = false;
            renderer.clock.reset();
            renderer.next_steps = [0; TRACK_COUNT];
            for voice in renderer
                .drums
                .iter_mut()
                .chain(renderer.preview_drums.iter_mut())
            {
                voice.envelope.value = DRUM_SILENCE;
                voice.envelope.elapsed = voice.envelope.decay_samples;
            }
            for v in renderer.synth.iter_mut().chain(renderer.preview.iter_mut()) {
                v.gate_off();
                v.active = false;
                v.remaining = 0;
            }
            for v in renderer
                .chord
                .voices
                .iter_mut()
                .chain(renderer.preview_chord.voices.iter_mut())
            {
                v.gate_off();
                v.active = false;
                v.remaining = 0;
            }
            renderer.chord.active = false;
            renderer.preview_chord.active = false;
            renderer.preview_activity = [false; TRACK_COUNT];
            renderer.chord.arpeggio = ArpeggioState::default();
            renderer.preview_chord.arpeggio = ArpeggioState::default();
            renderer.chord.chorus.clear();
            renderer.preview_chord.chorus.clear();
            renderer.delay.clear();
            renderer.reverb.clear();
            renderer.sidechain.reset();
            renderer.clear_track_effects();
            renderer.limiter.clear();
            renderer.reset_lfos();
            renderer.reset_trigger_state();
            renderer.status.running.store(false, Ordering::Release);
            renderer.status.paused.store(false, Ordering::Release);
            for playhead in &renderer.status.playheads {
                playhead.store(u8::MAX, Ordering::Release);
            }
            renderer.active_pattern = 0;
            renderer.queued_pattern = None;
            renderer.status.active_pattern.store(0, Ordering::Release);
            renderer
                .status
                .queued_pattern
                .store(u8::MAX, Ordering::Release);
        }
        AudioCommand::SelectPattern { pattern }
            if (pattern as usize) < renderer.project.patterns.len() =>
        {
            if renderer.playing {
                renderer.queued_pattern = Some(pattern as usize);
                renderer
                    .status
                    .queued_pattern
                    .store(pattern, Ordering::Release);
            } else {
                renderer.activate_pattern(pattern as usize);
            }
        }
        AudioCommand::SelectPattern { .. } => {}
        AudioCommand::ReplaceProject {
            project,
            smoothing,
            pattern_map,
        } => {
            renderer.replace_project(project, smoothing, pattern_map);
        }
        AudioCommand::Audition { track, step } => renderer.audition(track as usize, step as usize),
        AudioCommand::AutoAudition { track, step } if !renderer.playing => {
            renderer.audition(track as usize, step as usize)
        }
        AudioCommand::AutoAudition { .. } => {}
    }
}

impl Renderer {
    pub(super) fn command(&mut self, command: AudioCommand) {
        handle(self, command);
    }
}
