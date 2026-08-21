use super::{AudioCommand, AudioStatus, Renderer};
use crate::model::ParameterLocks;
use rtrb::Consumer;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl Renderer {
    pub(super) fn configure_effects(&mut self, smoothing_samples: u32) {
        self.clock.set_bpm(self.project.globals.tempo_bpm);
        self.sidechain.configure(self.project.globals.sidechain);
        self.delay.configure(
            self.project
                .globals
                .delay_division
                .samples(self.project.globals.tempo_bpm, self.sr as u32) as usize,
            self.project.globals.delay_feedback.normalized(),
        );
        self.reverb
            .set_time_smoothed(self.project.globals.reverb_time_seconds, smoothing_samples);
        self.reverb.set_tone_smoothed(
            self.project.globals.reverb_tone.normalized(),
            smoothing_samples,
        );
        self.reverb
            .set_pre_delay_smoothed(self.project.globals.reverb_pre_delay_ms, smoothing_samples);
        self.reverb_return.set(
            self.project.globals.reverb_return.normalized(),
            smoothing_samples,
        );
        for track in 0..super::TRACK_COUNT {
            self.effects[track].configure(
                self.project.tracks[track].effects,
                ParameterLocks::default(),
                smoothing_samples,
            );
            self.preview_effects[track].configure(
                self.project.tracks[track].effects,
                ParameterLocks::default(),
                smoothing_samples,
            );
            let controls = self.project.tracks[track].effects;
            for effect in self.voicing_effects[track]
                .iter_mut()
                .chain(self.preview_voicing_effects[track].iter_mut())
            {
                effect.configure(controls, ParameterLocks::default(), smoothing_samples);
            }
        }
    }

    pub(super) fn configure_track_effects(
        &mut self,
        track: usize,
        locks: ParameterLocks,
        smoothing_samples: u32,
        preview: bool,
    ) {
        let effects = self.project.tracks[track].effects;
        if self.project.tracks[track]
            .instrument_kind()
            .supports_voicing()
        {
            let chains = if preview {
                &mut self.preview_voicing_effects[track]
            } else {
                &mut self.voicing_effects[track]
            };
            for effect in chains {
                effect.configure(effects, locks, smoothing_samples);
            }
            return;
        }
        if preview {
            self.preview_effects[track].configure(effects, locks, smoothing_samples);
        } else {
            self.effects[track].configure(effects, locks, smoothing_samples);
        }
    }

    pub(super) fn clear_track_effects(&mut self) {
        for effect in self
            .effects
            .iter_mut()
            .chain(self.preview_effects.iter_mut())
            .chain(
                self.voicing_effects
                    .iter_mut()
                    .flat_map(|effects| effects.iter_mut()),
            )
            .chain(
                self.preview_voicing_effects
                    .iter_mut()
                    .flat_map(|effects| effects.iter_mut()),
            )
        {
            effect.clear();
        }
    }
    pub(super) fn update_mutes(&mut self, immediate: bool) {
        let smoothing = if immediate {
            0
        } else {
            (self.sr * 0.005) as u32
        };
        for (i, mute) in self.mute.iter_mut().enumerate() {
            mute.set((!self.project.tracks[i].muted) as u8 as f32, smoothing);
        }
    }
}

pub(super) fn render<T: Copy, F: Fn(f32) -> T>(
    out: &mut [T],
    channels: usize,
    sample_rate: u32,
    status: &AudioStatus,
    renderer: &mut Renderer,
    commands: &mut Consumer<AudioCommand>,
    convert: F,
) {
    let started = Instant::now();
    let had_pending = renderer.pending.is_some();
    if renderer.apply_pending() {
        let mut accumulated = None;
        let command_budget = super::MAX_COMMANDS_PER_CALLBACK - usize::from(had_pending);
        for _ in 0..command_budget {
            if accumulated.is_some() && renderer.retire.slots() == 0 {
                break;
            }
            let Ok(command) = commands.pop() else {
                break;
            };
            let coalescible = matches!(
                &command,
                AudioCommand::ReplaceProject {
                    pattern_map,
                    song_map,
                    ..
                } if pattern_map.is_identity() && song_map.is_identity()
            );
            let accumulated_is_coalescible = matches!(
                &accumulated,
                Some(AudioCommand::ReplaceProject {
                    pattern_map,
                    song_map,
                    ..
                }) if pattern_map.is_identity() && song_map.is_identity()
            );
            if coalescible && accumulated_is_coalescible && renderer.retire.slots() >= 2 {
                let Some(AudioCommand::ReplaceProject { project, .. }) = accumulated.take() else {
                    unreachable!()
                };
                renderer
                    .retire
                    .push(project)
                    .unwrap_or_else(|_| unreachable!());
                accumulated = Some(command);
                continue;
            }
            if let Some(previous) = accumulated.take() {
                renderer.command(previous);
            }
            if coalescible {
                accumulated = Some(command);
            } else {
                renderer.command(command);
            }
            if renderer.pending.is_some() {
                break;
            }
        }
        if let Some(command) = accumulated {
            renderer.command(command);
        }
    }
    for frame in out.chunks_mut(channels) {
        let (l, r) = renderer.next();
        renderer.recording.capture(l, r);
        if !frame.is_empty() {
            frame[0] = convert(if channels == 1 { (l + r) * 0.5 } else { l })
        }
        if channels > 1 {
            frame[1] = convert(r)
        }
        for sample in frame.iter_mut().skip(2) {
            *sample = convert(0.0)
        }
    }
    if !out.is_empty() && channels > 0 && sample_rate > 0 {
        let output_frames = out.len().div_ceil(channels);
        let budget = Duration::from_secs_f64(output_frames as f64 / sample_rate as f64);
        record_callback_timing(status, started.elapsed(), budget);
    }
}

fn record_callback_timing(status: &AudioStatus, elapsed: Duration, budget: Duration) {
    if budget.is_zero() {
        return;
    }
    let elapsed_ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    let budget_ns = budget.as_nanos().min(u64::MAX as u128) as u64;
    if budget_ns == 0 {
        return;
    }
    status
        .max_callback_duration_ns
        .fetch_max(elapsed_ns, Ordering::Relaxed);
    let load_per_mille =
        ((elapsed_ns as u128 * 1_000) / budget_ns as u128).min(u64::MAX as u128) as u64;
    status
        .max_callback_load_per_mille
        .fetch_max(load_per_mille, Ordering::Relaxed);
    if elapsed > budget {
        status.callback_overruns.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn modulated_percent(center: f32, offset: f32) -> f32 {
    (center + offset).clamp(0.0, 100.0)
}

pub(super) fn pitch_modulated_frequency(base_frequency: f32, offset_percent: f32) -> f32 {
    base_frequency * 2.0_f32.powf((offset_percent / 100.0 * 2.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::record_callback_timing;
    use crate::audio::AudioStatus;
    use std::{sync::atomic::Ordering, time::Duration};

    #[test]
    fn callback_telemetry_defaults_to_zero() {
        let status = AudioStatus::default();
        assert_eq!(status.callback_overruns.load(Ordering::Relaxed), 0);
        assert_eq!(status.max_callback_duration_ns.load(Ordering::Relaxed), 0);
        assert_eq!(
            status.max_callback_load_per_mille.load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn callback_telemetry_tracks_overruns_and_monotonic_maxima() {
        let status = AudioStatus::default();
        record_callback_timing(&status, Duration::from_millis(5), Duration::from_millis(10));
        assert_eq!(status.callback_overruns.load(Ordering::Relaxed), 0);
        assert_eq!(
            status.max_callback_duration_ns.load(Ordering::Relaxed),
            5_000_000
        );
        assert_eq!(
            status.max_callback_load_per_mille.load(Ordering::Relaxed),
            500
        );

        record_callback_timing(
            &status,
            Duration::from_millis(127),
            Duration::from_millis(100),
        );
        assert_eq!(status.callback_overruns.load(Ordering::Relaxed), 1);
        assert_eq!(
            status.max_callback_duration_ns.load(Ordering::Relaxed),
            127_000_000
        );
        assert_eq!(
            status.max_callback_load_per_mille.load(Ordering::Relaxed),
            1_270
        );

        record_callback_timing(
            &status,
            Duration::from_millis(20),
            Duration::from_millis(100),
        );
        assert_eq!(status.callback_overruns.load(Ordering::Relaxed), 1);
        assert_eq!(
            status.max_callback_duration_ns.load(Ordering::Relaxed),
            127_000_000
        );
        assert_eq!(
            status.max_callback_load_per_mille.load(Ordering::Relaxed),
            1_270
        );
    }
}
