use super::voices::ArpeggioState;
use super::{
    AudioProject, AudioTrack, ParameterSmoothing, Renderer, SCHEDULED_ACTION_COUNT,
    ScheduledTrackAction, TRACK_COUNT,
};
use crate::dsp::Lfo;
use crate::model::{
    MAX_STEP_COUNT, ParameterId, ParameterLocks, PatternIndexMap, SongIndexMap, TriggerCondition,
};
use std::sync::atomic::Ordering;

impl Renderer {
    fn reset_track_runtime(&mut self, track: usize) {
        for voice in [&mut self.drums[track], &mut self.preview_drums[track]] {
            voice.reset_runtime();
        }
        for voice in [&mut self.synth[track], &mut self.preview[track]] {
            voice.gate_off();
            voice.reset_to_idle();
        }
        for pool in [&mut self.voicings[track], &mut self.preview_voicings[track]] {
            for voice in &mut pool.voices {
                voice.gate_off();
                voice.reset_to_idle();
            }
            pool.active = false;
            pool.arpeggiated = false;
            pool.group_voice_counts = [0; 2];
            pool.arpeggio = ArpeggioState::default();
            pool.preview_remaining = 0;
        }
        self.effects[track].clear();
        self.preview_effects[track].clear();
        for effect in self.voicing_effects[track]
            .iter_mut()
            .chain(self.preview_voicing_effects[track].iter_mut())
        {
            effect.clear();
        }
        for lfo in self.lfos[track]
            .iter_mut()
            .chain(self.preview_lfos[track].iter_mut())
        {
            lfo.reset();
        }
        self.lfo_offsets[track] = [0.0; ParameterId::ALL.len()];
        self.preview_lfo_offsets[track] = [0.0; ParameterId::ALL.len()];
        self.preview_activity[track] = false;
        self.next_steps[track] = 0;
        self.early_armed[track] = None;
        self.cycle_counts[track] = [0; MAX_STEP_COUNT];
        self.condition_rng[track] = 0x8a5c_9d31 ^ (track as u32).wrapping_mul(0x9e37_79b9);
        self.probability_rng[track] = 0x3c6e_f372 ^ (track as u32).wrapping_mul(0x7f4a_7c15);
        for action in &mut self.scheduled {
            if action.is_some_and(|action| usize::from(action.track) == track) {
                *action = None;
            }
        }
        for action in &mut self.preview_scheduled {
            if action.is_some_and(|action| usize::from(action.track) == track) {
                *action = None;
            }
        }
        self.status.playheads[track].store(u8::MAX, Ordering::Release);
    }

    pub(super) fn reset_trigger_state(&mut self) {
        self.scheduled = [None; SCHEDULED_ACTION_COUNT];
        self.early_armed = [None; TRACK_COUNT];
        self.preview_scheduled = [None; 24];
        self.cycle_counts = [[0; MAX_STEP_COUNT]; TRACK_COUNT];
        self.condition_rng =
            std::array::from_fn(|i| 0x8a5c_9d31 ^ (i as u32).wrapping_mul(0x9e37_79b9));
        self.probability_rng =
            std::array::from_fn(|i| 0x3c6e_f372 ^ (i as u32).wrapping_mul(0x7f4a_7c15));
    }
    pub(super) fn condition_passes(
        &mut self,
        track: usize,
        step: usize,
        condition: TriggerCondition,
    ) -> bool {
        match condition {
            TriggerCondition::Always => true,
            TriggerCondition::Cycle { position, length } => {
                let count = self.cycle_counts[track][step];
                self.cycle_counts[track][step] = count.wrapping_add(1);
                (count % u32::from(length)) + 1 == u32::from(position)
            }
            TriggerCondition::Chance { probability } => {
                let state = &mut self.condition_rng[track];
                *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*state % 100) < u32::from(probability.get())
            }
        }
    }
    pub(super) fn probability_passes(&mut self, track: usize) -> bool {
        let probability = self.project.tracks[track].probability.get();
        if probability == 0 {
            return false;
        }
        if probability == 100 {
            return true;
        }
        let state = &mut self.probability_rng[track];
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*state % 100) < u32::from(probability)
    }
    pub(super) fn enqueue(&mut self, action: ScheduledTrackAction) {
        if let Some(slot) = self.scheduled.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(action);
        }
    }
    pub(super) fn prune_scheduled_actions(&mut self) {
        for action in &mut self.scheduled {
            let Some(pending) = *action else { continue };
            let valid = self
                .project
                .patterns
                .get(self.active_pattern)
                .and_then(|pattern| pattern.tracks.get(usize::from(pending.track)))
                .is_some_and(|sequence| usize::from(pending.step) < sequence.step_count as usize);
            if !valid {
                *action = None;
            }
        }
    }
    pub(super) fn invalidate_replaced_scheduled_actions(
        scheduled: &mut [Option<ScheduledTrackAction>; SCHEDULED_ACTION_COUNT],
        old: &AudioProject,
        old_active_pattern: usize,
        next: &AudioProject,
        next_active_pattern: usize,
    ) {
        for action in scheduled {
            let Some(pending) = *action else { continue };
            let track = usize::from(pending.track);
            let step = usize::from(pending.step);
            let old_event = old
                .patterns
                .get(old_active_pattern)
                .and_then(|pattern| pattern.tracks.get(track))
                .and_then(|sequence| sequence.steps.get(step).copied().flatten());
            let next_event = next
                .patterns
                .get(next_active_pattern)
                .and_then(|pattern| pattern.tracks.get(track))
                .and_then(|sequence| sequence.steps.get(step).copied().flatten());
            if old_event != next_event {
                *action = None;
            }
        }
    }
    pub(super) fn advance_scheduled(&mut self) {
        self.prune_scheduled_actions();
        let mut ready = [None; SCHEDULED_ACTION_COUNT];
        for (index, action) in self.scheduled.iter_mut().enumerate() {
            let Some(mut pending) = *action else { continue };
            if pending.remaining > 0 {
                pending.remaining -= 1;
                *action = Some(pending);
            } else {
                ready[index] = Some(pending);
                *action = None;
            }
        }
        for action in ready.into_iter().flatten() {
            self.process_track_action(
                action.track as usize,
                action.step as usize,
                action.retrigger,
                action.trigger_allowed,
            );
        }
    }
    pub(super) fn advance_preview_scheduled(&mut self) {
        let mut ready = [None; 24];
        for (index, action) in self.preview_scheduled.iter_mut().enumerate() {
            let Some(mut pending) = *action else {
                continue;
            };
            if pending.remaining > 0 {
                pending.remaining -= 1;
                *action = Some(pending);
            } else {
                ready[index] = Some(pending);
                *action = None;
            }
        }
        for action in ready.into_iter().flatten() {
            self.audition_once(action.track as usize, action.step as usize);
        }
    }

    pub(super) fn replace_project(
        &mut self,
        project: Box<AudioProject>,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
        song_map: SongIndexMap,
    ) {
        if self.retire.slots() == 0 {
            self.pending = Some((project, smoothing, pattern_map, song_map));
            return;
        }
        let old_active_pattern = self.active_pattern;
        let active_pattern = pattern_map.rebase(self.active_pattern, project.patterns.len());
        let queued_pattern = self
            .queued_pattern
            .map(|index| pattern_map.rebase(index, project.patterns.len()));
        let active_song_removed = song_map.removes(self.active_song);
        let active_song = song_map.rebase(self.active_song, project.song.len());
        let queued_song = self
            .queued_song
            .map(|index| song_map.rebase(index, project.song.len()));
        let song_bar = if active_song_removed {
            0
        } else {
            self.song_bar
                .min(project.song[active_song].bars.saturating_sub(1))
        };
        let changed_instruments: [bool; TRACK_COUNT] = std::array::from_fn(|track| {
            self.project.tracks[track].instrument_kind() != project.tracks[track].instrument_kind()
        });
        self.reconcile_lfos(&project);
        Self::invalidate_replaced_scheduled_actions(
            &mut self.scheduled,
            &self.project,
            old_active_pattern,
            &project,
            active_pattern,
        );
        for track in 0..TRACK_COUNT {
            let Some(step) = self.early_armed[track].map(usize::from) else {
                continue;
            };
            let old_event = self
                .project
                .patterns
                .get(old_active_pattern)
                .and_then(|pattern| pattern.tracks.get(track))
                .and_then(|sequence| sequence.steps.get(step).copied().flatten());
            let next_event = project
                .patterns
                .get(active_pattern)
                .and_then(|pattern| pattern.tracks.get(track))
                .and_then(|sequence| sequence.steps.get(step).copied().flatten());
            if old_active_pattern != active_pattern || old_event != next_event {
                self.early_armed[track] = None;
            }
        }
        for (track, changed) in changed_instruments.into_iter().enumerate() {
            if changed {
                self.reset_track_runtime(track);
            }
        }
        let old = std::mem::replace(&mut self.project, project);
        match self.retire.push(old) {
            Ok(()) => {}
            // `slots()` was checked above and this producer is callback-local;
            // a full queue here indicates an internal invariant violation.
            Err(_) => unreachable!(),
        }
        self.active_pattern = active_pattern;
        self.queued_pattern = queued_pattern;
        self.active_song = active_song;
        self.queued_song = queued_song;
        self.song_bar = song_bar;
        self.prune_scheduled_actions();
        self.rebuild_lfo_destinations();
        self.status
            .active_pattern
            .store(active_pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(
            queued_pattern.map_or(u8::MAX, |pattern| pattern as u8),
            Ordering::Release,
        );
        self.status
            .active_song
            .store(active_song as u8, Ordering::Release);
        self.status.queued_song.store(
            queued_song.map_or(u16::MAX, |entry| entry as u16),
            Ordering::Release,
        );
        self.status.song_bar.store(self.song_bar, Ordering::Release);
        let smoothing_samples = smoothing.samples(self.sr);
        for (track, next) in self.next_steps.iter_mut().enumerate() {
            if *next >= self.project.patterns[self.active_pattern].tracks[track].step_count as usize
            {
                *next = 0;
            }
            let playhead = &self.status.playheads[track];
            if playhead.load(Ordering::Acquire)
                >= self.project.patterns[self.active_pattern].tracks[track].step_count
            {
                playhead.store(u8::MAX, Ordering::Release);
            }
        }
        self.configure_effects(smoothing_samples);
        self.update_mutes(false);
        self.refresh_active_parameters(smoothing_samples);
    }

    pub(super) fn apply_pending(&mut self) -> bool {
        let Some((project, smoothing, pattern_map, song_map)) = self.pending.take() else {
            return true;
        };
        self.replace_project(project, smoothing, pattern_map, song_map);
        self.pending.is_none()
    }

    pub(super) fn activate_pattern(&mut self, pattern: usize) {
        self.active_pattern = pattern;
        self.queued_pattern = None;
        self.next_steps = [0; TRACK_COUNT];
        self.reset_trigger_state();
        self.status
            .active_pattern
            .store(pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(u8::MAX, Ordering::Release);
    }

    pub(super) fn activate_song(&mut self, entry: usize) {
        let entry = entry.min(self.project.song.len().saturating_sub(1));
        self.song_mode = true;
        self.active_song = entry;
        self.queued_song = None;
        self.song_bar = 0;
        self.activate_pattern(usize::from(self.project.song[entry].pattern - 1));
        self.status.song_mode.store(true, Ordering::Release);
        self.status
            .active_song
            .store(entry as u8, Ordering::Release);
        self.status.queued_song.store(u16::MAX, Ordering::Release);
        self.status.song_bar.store(0, Ordering::Release);
    }

    pub(super) fn advance_song(&mut self) -> bool {
        let entry = self.project.song[self.active_song];
        if self.song_bar + 1 < entry.bars {
            self.song_bar += 1;
            let pattern = usize::from(entry.pattern - 1);
            if self.active_pattern != pattern {
                self.activate_pattern(pattern);
            }
            self.status.song_bar.store(self.song_bar, Ordering::Release);
            true
        } else if self.active_song + 1 < self.project.song.len() {
            self.activate_song(self.active_song + 1);
            true
        } else {
            false
        }
    }

    pub(super) fn reset_lfos(&mut self) {
        for lfo in self.lfos.iter_mut().flatten() {
            lfo.reset();
        }
        for lfo in self.preview_lfos.iter_mut().flatten() {
            lfo.reset();
        }
        self.lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
        self.preview_lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
    }

    pub(super) fn reconcile_lfos(&mut self, next: &AudioProject) {
        for track in 0..TRACK_COUNT {
            for parameter in ParameterId::ALL {
                let old_enabled = self.project.tracks[track]
                    .lfos
                    .get(parameter)
                    .is_some_and(|config| config.enabled);
                let new_enabled = next.tracks[track]
                    .lfos
                    .get(parameter)
                    .is_some_and(|config| config.enabled);
                if !old_enabled && new_enabled {
                    self.lfos[track][parameter as usize].reset();
                    self.preview_lfos[track][parameter as usize].reset();
                } else if !new_enabled {
                    self.lfos[track][parameter as usize].disable();
                    self.preview_lfos[track][parameter as usize].disable();
                    self.lfo_offsets[track][parameter as usize] = 0.0;
                    self.preview_lfo_offsets[track][parameter as usize] = 0.0;
                }
            }
        }
    }

    pub(super) fn rebuild_lfo_destinations(&mut self) {
        for track in 0..TRACK_COUNT {
            let mut count = 0;
            for parameter in ParameterId::ALL {
                if self.project.tracks[track]
                    .lfos
                    .get(parameter)
                    .is_some_and(|config| config.enabled)
                {
                    self.lfo_destinations[track][count] = parameter;
                    count += 1;
                }
            }
            self.lfo_destination_count[track] = count as u8;
        }
    }

    pub(super) fn advance_lfo_bank(
        states: &mut [Lfo; ParameterId::ALL.len()],
        offsets: &mut [f32; ParameterId::ALL.len()],
        destinations: &[ParameterId; ParameterId::ALL.len()],
        destination_count: u8,
        track: AudioTrack,
        tempo_bpm: u16,
        sample_rate: f32,
    ) {
        for &parameter in destinations.iter().take(destination_count as usize) {
            let config = track.lfos.get(parameter);
            let value = states[parameter as usize].next(config, tempo_bpm, sample_rate);
            offsets[parameter as usize] =
                value * config.map_or(0.0, |config| config.depth.get() as f32);
        }
    }

    pub(super) fn restart_trigger_lfo_bank(
        states: &mut [Lfo; ParameterId::ALL.len()],
        offsets: &mut [f32; ParameterId::ALL.len()],
        destinations: &[ParameterId; ParameterId::ALL.len()],
        destination_count: u8,
        track: AudioTrack,
        tempo_bpm: u16,
        sample_rate: f32,
    ) {
        for &parameter in destinations.iter().take(destination_count as usize) {
            let Some(config) = track
                .lfos
                .get(parameter)
                .filter(|config| config.enabled && config.reset_on_trigger)
            else {
                continue;
            };
            let value = states[parameter as usize].restart(config, tempo_bpm, sample_rate);
            offsets[parameter as usize] = value * config.depth.get() as f32;
        }
    }

    pub(super) fn restart_trigger_lfos(&mut self, track: usize) {
        Self::restart_trigger_lfo_bank(
            &mut self.lfos[track],
            &mut self.lfo_offsets[track],
            &self.lfo_destinations[track],
            self.lfo_destination_count[track],
            self.project.tracks[track],
            self.project.globals.tempo_bpm,
            self.sr,
        );
    }

    pub(super) fn restart_preview_trigger_lfos(&mut self, track: usize) {
        Self::restart_trigger_lfo_bank(
            &mut self.preview_lfos[track],
            &mut self.preview_lfo_offsets[track],
            &self.lfo_destinations[track],
            self.lfo_destination_count[track],
            self.project.tracks[track],
            self.project.globals.tempo_bpm,
            self.sr,
        );
    }

    pub(super) fn advance_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            Self::advance_lfo_bank(
                &mut self.lfos[track],
                &mut self.lfo_offsets[track],
                &self.lfo_destinations[track],
                self.lfo_destination_count[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    pub(super) fn advance_preview_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            if !self.preview_activity[track] {
                continue;
            }
            Self::advance_lfo_bank(
                &mut self.preview_lfos[track],
                &mut self.preview_lfo_offsets[track],
                &self.lfo_destinations[track],
                self.lfo_destination_count[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    pub(super) fn reset_preview_lfos(&mut self, track: usize) {
        for lfo in &mut self.preview_lfos[track] {
            lfo.reset();
        }
        Self::advance_lfo_bank(
            &mut self.preview_lfos[track],
            &mut self.preview_lfo_offsets[track],
            &self.lfo_destinations[track],
            self.lfo_destination_count[track],
            self.project.tracks[track],
            self.project.globals.tempo_bpm,
            self.sr,
        );
    }

    pub(super) fn locks_at(&self, track: usize, step: usize) -> ParameterLocks {
        let t = self.project.patterns[self.active_pattern].tracks[track];
        crate::model::effective_event_locks(&t.steps[..t.step_count as usize], step)
            .unwrap_or_default()
    }
}
