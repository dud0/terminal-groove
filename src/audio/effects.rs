use super::{AudioCommand, Renderer};
use rtrb::Consumer;

impl Renderer {
    pub(super) fn configure_effects(&mut self, smoothing_samples: u32) {
        self.clock.set_bpm(self.project.globals.tempo_bpm);
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
    renderer: &mut Renderer,
    commands: &mut Consumer<AudioCommand>,
    convert: F,
) {
    if renderer.apply_pending() {
        while let Ok(c) = commands.pop() {
            let is_replace = matches!(c, AudioCommand::ReplaceProject { .. });
            renderer.command(c);
            if is_replace && renderer.pending.is_some() {
                break;
            }
        }
    }
    for frame in out.chunks_mut(channels) {
        let (l, r) = renderer.next();
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
}

pub(super) fn modulated_percent(center: f32, offset: f32) -> f32 {
    (center + offset).clamp(0.0, 100.0)
}

pub(super) fn pitch_modulated_frequency(base_frequency: f32, offset_percent: f32) -> f32 {
    base_frequency * 2.0_f32.powf((offset_percent / 100.0 * 2.0) / 12.0)
}
