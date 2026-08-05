#[allow(unused_imports)]
use super::*;

impl Renderer {
    pub(crate) fn reset_trigger_state(&mut self) {
        self.scheduled = [None; 32];
        self.preview_scheduled = [None; 24];
        self.cycle_counts = [[0; MAX_STEP_COUNT]; TRACK_COUNT];
        self.condition_rng =
            std::array::from_fn(|i| 0x8a5c_9d31 ^ (i as u32).wrapping_mul(0x9e37_79b9));
    }
    pub(crate) fn condition_passes(
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
    pub(crate) fn enqueue(&mut self, action: ScheduledTrackAction) {
        if let Some(slot) = self.scheduled.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(action);
        }
    }
    pub(crate) fn prune_scheduled_actions(&mut self) {
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
    pub(crate) fn invalidate_replaced_scheduled_actions(
        scheduled: &mut [Option<ScheduledTrackAction>; 32],
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
    pub(crate) fn advance_scheduled(&mut self) {
        self.prune_scheduled_actions();
        let mut ready = [None; 32];
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
    pub(crate) fn advance_preview_scheduled(&mut self) {
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

    pub(crate) fn replace_project(
        &mut self,
        project: Box<AudioProject>,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    ) {
        if self.retire.slots() == 0 {
            self.pending = Some((project, smoothing, pattern_map));
            return;
        }
        let old_active_pattern = self.active_pattern;
        let active_pattern = pattern_map.rebase(self.active_pattern, project.patterns.len());
        let queued_pattern = self
            .queued_pattern
            .map(|index| pattern_map.rebase(index, project.patterns.len()));
        self.reconcile_lfos(&project);
        Self::invalidate_replaced_scheduled_actions(
            &mut self.scheduled,
            &self.project,
            old_active_pattern,
            &project,
            active_pattern,
        );
        let old = std::mem::replace(&mut self.project, project);
        debug_assert!(self.retire.push(old).is_ok());
        self.active_pattern = active_pattern;
        self.queued_pattern = queued_pattern;
        self.prune_scheduled_actions();
        self.status
            .active_pattern
            .store(active_pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(
            queued_pattern.map_or(u8::MAX, |pattern| pattern as u8),
            Ordering::Release,
        );
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

    pub(crate) fn apply_pending(&mut self) -> bool {
        let Some((project, smoothing, map)) = self.pending.take() else {
            return true;
        };
        self.replace_project(project, smoothing, map);
        self.pending.is_none()
    }

    pub(crate) fn activate_pattern(&mut self, pattern: usize) {
        self.active_pattern = pattern;
        self.queued_pattern = None;
        self.next_steps = [0; TRACK_COUNT];
        self.reset_trigger_state();
        self.status
            .active_pattern
            .store(pattern as u8, Ordering::Release);
        self.status.queued_pattern.store(u8::MAX, Ordering::Release);
    }

    pub(crate) fn reset_lfos(&mut self) {
        for lfo in self.lfos.iter_mut().flatten() {
            lfo.reset();
        }
        for lfo in self.preview_lfos.iter_mut().flatten() {
            lfo.reset();
        }
        self.lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
        self.preview_lfo_offsets = [[0.0; ParameterId::ALL.len()]; TRACK_COUNT];
    }

    pub(crate) fn reconcile_lfos(&mut self, next: &AudioProject) {
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

    pub(super) fn advance_lfo_bank(
        states: &mut [Lfo; ParameterId::ALL.len()],
        offsets: &mut [f32; ParameterId::ALL.len()],
        track: AudioTrack,
        tempo_bpm: u16,
        sample_rate: f32,
    ) {
        for parameter in ParameterId::ALL {
            let config = track.lfos.get(parameter);
            let value = states[parameter as usize].next(config, tempo_bpm, sample_rate);
            offsets[parameter as usize] =
                value * config.map_or(0.0, |config| config.depth.get() as f32);
        }
    }

    pub(crate) fn advance_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            Self::advance_lfo_bank(
                &mut self.lfos[track],
                &mut self.lfo_offsets[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    pub(crate) fn advance_preview_lfos(&mut self) {
        let tempo = self.project.globals.tempo_bpm;
        for track in 0..TRACK_COUNT {
            Self::advance_lfo_bank(
                &mut self.preview_lfos[track],
                &mut self.preview_lfo_offsets[track],
                self.project.tracks[track],
                tempo,
                self.sr,
            );
        }
    }

    pub(crate) fn reset_preview_lfos(&mut self, track: usize) {
        for lfo in &mut self.preview_lfos[track] {
            lfo.reset();
        }
        Self::advance_lfo_bank(
            &mut self.preview_lfos[track],
            &mut self.preview_lfo_offsets[track],
            self.project.tracks[track],
            self.project.globals.tempo_bpm,
            self.sr,
        );
    }

    pub(crate) fn locks_at(&self, track: usize, step: usize) -> ParameterLocks {
        let t = self.project.patterns[self.active_pattern].tracks[track];
        let Some(event) = t.steps[step] else {
            return ParameterLocks::default();
        };
        let mut locks = *event.locks();
        if let StepEvent::Tie { .. } = event {
            if let Some(source) = crate::model::tie_source(&t.steps[..t.step_count as usize], step)
            {
                if let Some(
                    StepEvent::Note {
                        locks: source_locks,
                        ..
                    }
                    | StepEvent::BassNote {
                        locks: source_locks,
                        ..
                    },
                ) = t.steps[source]
                {
                    locks = source_locks;
                    let mut i = (source + 1) % t.step_count as usize;
                    while i != step {
                        if let Some(StepEvent::Tie { locks: tie_locks }) = t.steps[i] {
                            locks.overlay(tie_locks);
                        }
                        i = (i + 1) % t.step_count as usize;
                    }
                    locks.overlay(*event.locks());
                }
            }
        }
        locks
    }
}
