#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn osc_bounded() {
        let mut o = PolyBlepOsc::default();
        for _ in 0..10000 {
            assert!(o.next_saw(440., 48000.).abs() <= 1.1);
        }
    }

    #[test]
    fn additive_source_mapping_keeps_both_sources_at_the_midpoint() {
        let (pulse, saw) = additive_source_gains(0.5);
        assert!(pulse > 0.0 && saw > 0.0);
        assert_eq!(additive_source_gains(0.0), (1.0, 0.0));
        assert_eq!(additive_source_gains(1.0), (0.0, 1.0));
    }

    #[test]
    fn voice_local_noise_is_deterministic_finite_and_seeded() {
        let mut first = NoiseSource::new(0x1234_5678);
        let mut second = NoiseSource::new(0x1234_5678);
        let mut different = NoiseSource::new(0x8765_4321);
        let a: Vec<_> = (0..32).map(|_| first.next_sample()).collect();
        let b: Vec<_> = (0..32).map(|_| second.next_sample()).collect();
        let c: Vec<_> = (0..32).map(|_| different.next_sample()).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(
            a.iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.01)
        );
    }

    #[test]
    fn chord_and_lead_filters_have_distinct_resonant_responses() {
        let mut chord = ChordFilter::new();
        let mut lead = LeadFilter::new();
        chord.set_parameters_smoothed(1_200.0, 0.75, 96_000.0, 0);
        lead.set_parameters_smoothed(1_200.0, 0.75, 96_000.0, 0);
        let mut chord_energy = 0.0;
        let mut lead_energy = 0.0;
        for sample in 0..512 {
            let input = if sample == 0 { 1.0 } else { 0.0 };
            chord_energy += chord.process(input).abs();
            lead_energy += lead.process(input).abs();
        }
        assert!((chord_energy - lead_energy).abs() > 0.001);
        assert!(chord_energy.is_finite() && lead_energy.is_finite());
    }

    #[test]
    fn calibrated_four_pole_resonance_emphasizes_cutoff_relative_to_the_low_end() {
        enum Filter {
            Chord(ChordFilter),
            Lead(LeadFilter),
        }

        impl Filter {
            fn set_parameters(&mut self, cutoff: f32, resonance: f32, sample_rate: f32) {
                match self {
                    Self::Chord(filter) => {
                        filter.set_parameters_smoothed(cutoff, resonance, sample_rate, 0)
                    }
                    Self::Lead(filter) => {
                        filter.set_parameters_smoothed(cutoff, resonance, sample_rate, 0)
                    }
                }
            }

            fn process(&mut self, input: f32) -> f32 {
                match self {
                    Self::Chord(filter) => filter.process(input),
                    Self::Lead(filter) => filter.process(input),
                }
            }
        }

        fn rms_at(kind: &str, resonance: f32, frequency: f32) -> f32 {
            let sample_rate = 96_000.0;
            let cutoff = 1_000.0;
            let mut filter = match kind {
                "Chord" => Filter::Chord(ChordFilter::new()),
                "Lead" => Filter::Lead(LeadFilter::new()),
                _ => unreachable!(),
            };
            filter.set_parameters(cutoff, resonance, sample_rate);
            let mut energy = 0.0;
            let mut count = 0;
            for sample in 0..96_000 {
                let input = (std::f32::consts::TAU * frequency * sample as f32 / sample_rate).sin();
                let output = filter.process(input);
                assert!(output.is_finite(), "{kind} produced a non-finite output");
                if sample >= 48_000 {
                    energy += output * output;
                    count += 1;
                }
            }
            (energy / count as f32).sqrt()
        }

        for (kind, cutoff_region) in [("Chord", 920.0), ("Lead", 1_060.0)] {
            let low_at_zero = rms_at(kind, 0.0, 100.0);
            let low_at_maximum = rms_at(kind, 1.0, 100.0);
            let cutoff_at_zero = rms_at(kind, 0.0, cutoff_region);
            let cutoff_at_maximum = rms_at(kind, 1.0, cutoff_region);
            let high_at_maximum = rms_at(kind, 1.0, cutoff_region * 4.0);
            let ratio_at_zero = cutoff_at_zero / low_at_zero;
            let ratio_at_maximum = cutoff_at_maximum / low_at_maximum;

            assert!(
                ratio_at_maximum > ratio_at_zero,
                "{kind} resonance did not emphasize cutoff relative to the low end: \
                 {ratio_at_zero:.3} -> {ratio_at_maximum:.3}"
            );
            assert!(
                high_at_maximum < low_at_maximum * 0.5,
                "{kind} no longer has a four-pole high-frequency rolloff"
            );
        }
    }

    #[test]
    fn calibrated_four_pole_maximum_resonance_decays_after_an_impulse() {
        fn assert_impulse_decays(mut process: impl FnMut(f32) -> f32) {
            let mut early_peak = 0.0_f32;
            let mut tail_peak = 0.0_f32;
            for sample in 0..48_000 {
                let output = process(if sample == 0 { 1.0 } else { 0.0 });
                assert!(output.is_finite());
                if sample < 4_000 {
                    early_peak = early_peak.max(output.abs());
                } else if sample >= 44_000 {
                    tail_peak = tail_peak.max(output.abs());
                }
            }
            assert!(early_peak > 0.0);
            assert!(
                tail_peak < early_peak * 0.001,
                "impulse tail did not decay: {early_peak:.6} -> {tail_peak:.6}"
            );
        }

        let mut chord = ChordFilter::new();
        chord.set_parameters_smoothed(1_000.0, 1.0, 96_000.0, 0);
        assert_impulse_decays(|input| chord.process(input));

        let mut filter = LeadFilter::new();
        filter.set_parameters_smoothed(1_000.0, 1.0, 96_000.0, 0);
        assert_impulse_decays(|input| filter.process(input));
    }
    #[test]
    fn drum_biquad_filters_are_finite_and_reject_dc() {
        for configure in [
            Biquad::set_bandpass as fn(&mut Biquad, f32, f32, f32),
            Biquad::set_highpass,
        ] {
            let mut filter = Biquad::new();
            configure(&mut filter, 6_000.0, 5.6, 48_000.0);
            let mut output = 0.0;
            for _ in 0..48_000 {
                output = filter.process(1.0);
                assert!(output.is_finite());
            }
            assert!(output.abs() < 0.0001);
        }
    }
    #[test]
    fn reverb_highpass_rejects_dc_after_the_initial_transient() {
        let mut reverb = Reverb::new(8_000);
        let mut output = (0.0, 0.0);
        for _ in 0..40_000 {
            output = reverb.process(1.0, 1.0);
            assert!(output.0.is_finite() && output.1.is_finite());
        }
        assert!(output.0.abs().max(output.1.abs()) < 0.0001, "{output:?}");
    }
    #[test]
    fn delay_exact() {
        let mut d = Delay::new(100);
        d.configure(10, 0.);
        d.process(1., 0.);
        for _ in 0..9 {
            assert_eq!(d.process(0., 0.).0, 0.)
        }
        assert_eq!(d.process(0., 0.).0, 1.);
    }

    #[test]
    fn silent_effects_do_not_advance_when_inactive() {
        let mut chorus = StereoChorus::new(1_000);
        chorus.process(0.0);
        assert_eq!(chorus.pos, 0);

        let mut delay = Delay::new(1_000);
        delay.process(0.0, 0.0);
        assert_eq!(delay.pos, 0);

        let mut reverb = Reverb::new(1_000);
        reverb.process(0.0, 0.0);
        assert_eq!(reverb.pre_delay_pos, 0);
    }
    #[test]
    fn nonfinite_safe() {
        assert_eq!(safety(f32::NAN), 0.0);
        assert_eq!(safety(100.0), 100.0)
    }
    #[test]
    fn master_limiter_is_stereo_linked_and_respects_ceiling() {
        let mut limiter = MasterLimiter::new(1_000);
        let mut peak = 0.0_f32;
        let mut linked = false;
        for i in 0..500 {
            let input = if i == 20 { (8.0, 0.25) } else { (0.25, 0.25) };
            let (l, r) = limiter.process(input.0, input.1);
            peak = peak.max(l.abs()).max(r.abs());
            linked |= (l - r).abs() < 0.0001 && l.abs() > 0.01;
        }
        assert!(peak <= 0.891_251);
        assert!(linked);
    }
    struct NaiveLimiter {
        left: Vec<f32>,
        right: Vec<f32>,
        pos: usize,
        gain: f32,
        attack_coefficient: f32,
        release_coefficient: f32,
    }

    impl NaiveLimiter {
        fn new(sample_rate: u32) -> Self {
            let lookahead = ((sample_rate as f32 * 0.005).round() as usize).max(1);
            Self {
                left: vec![0.0; lookahead],
                right: vec![0.0; lookahead],
                pos: 0,
                gain: 1.0,
                attack_coefficient: (-1.0 / (sample_rate as f32 * 0.001)).exp(),
                release_coefficient: (-1.0 / (sample_rate as f32 * 0.080)).exp(),
            }
        }

        fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
            const MAKEUP: f32 = 1.995_262_3;
            const CEILING: f32 = 0.891_250_9;
            let delayed_l = self.left[self.pos];
            let delayed_r = self.right[self.pos];
            self.left[self.pos] = safety(l);
            self.right[self.pos] = safety(r);
            self.pos = (self.pos + 1) % self.left.len();
            let peak = self
                .left
                .iter()
                .chain(&self.right)
                .fold(0.0_f32, |peak, sample| peak.max((sample * MAKEUP).abs()));
            let target = if peak > CEILING { CEILING / peak } else { 1.0 };
            let coefficient = if target < self.gain {
                self.attack_coefficient
            } else {
                self.release_coefficient
            };
            self.gain = target + coefficient * (self.gain - target);
            let l = (delayed_l * MAKEUP * self.gain).clamp(-CEILING, CEILING);
            let r = (delayed_r * MAKEUP * self.gain).clamp(-CEILING, CEILING);
            (safety(l), safety(r))
        }

        fn clear(&mut self) {
            self.left.fill(0.0);
            self.right.fill(0.0);
            self.pos = 0;
            self.gain = 1.0;
        }
    }

    #[test]
    fn master_limiter_matches_naive_sliding_window() {
        for sample_rate in [8_000, 44_100, 48_000] {
            let mut optimized = MasterLimiter::new(sample_rate);
            let mut reference = NaiveLimiter::new(sample_rate);
            let window = ((sample_rate as f32 * 0.005).round() as usize).max(1);
            for i in 0..(window * 4 + 19) {
                let input = match i {
                    0..=2 => (0.0, 0.0),
                    3 => (8.0, 0.25),
                    4 => (0.1, 0.9),
                    i if (5..=window + 2).contains(&i) => (0.35, 0.35),
                    _ if i % 11 == 0 => (-0.7, 0.2),
                    _ => (0.0, 0.0),
                };
                let actual = optimized.process(input.0, input.1);
                let expected = reference.process(input.0, input.1);
                assert!((actual.0 - expected.0).abs() < 0.000001, "left at {i}");
                assert!((actual.1 - expected.1).abs() < 0.000001, "right at {i}");

                if i == window + 7 {
                    optimized.clear();
                    reference.clear();
                }
            }
        }
    }

    #[test]
    fn smoother_reaches_target_without_jump() {
        let mut s = Smoother::new(0.0);
        s.set(1.0, 10);
        let values = (0..10).map(|_| s.next_value()).collect::<Vec<_>>();
        assert!((values[0] - 0.1).abs() < 0.0001);
        assert!((values[9] - 1.0).abs() < 0.0001);
        assert!(values.windows(2).all(|w| w[1] >= w[0]));
    }
    #[test]
    fn comb_feedback_reaches_minus_sixty_db_at_requested_time() {
        for sample_rate in [8_000.0_f32, 44_100.0, 48_000.0] {
            for time in [0.2_f32, 2.5, 10.0] {
                let delay_samples = (sample_rate * 0.025).round() as usize;
                let mut comb = Comb::new(delay_samples);
                comb.set_time(time, sample_rate, 0);
                let delay_seconds = comb.buffer.len() as f32 / sample_rate;
                let loops = time / delay_seconds;
                let decay = comb.feedback.current.powf(loops);
                assert!(
                    (decay - 0.001).abs() < 0.00001,
                    "{sample_rate} Hz, {time} s"
                );
            }
        }
    }
    #[test]
    fn comb_feedback_changes_smoothly() {
        let mut comb = Comb::new(100);
        comb.set_time(0.2, 1_000.0, 0);
        let initial = comb.feedback.current;
        comb.set_time(10.0, 1_000.0, 10);
        let target = comb.feedback.target;

        comb.process(0.0, 0.25);
        assert!(comb.feedback.current > initial && comb.feedback.current < target);
        for _ in 1..10 {
            comb.process(0.0, 0.25);
        }
        assert_eq!(comb.feedback.current, target);
    }
    #[test]
    fn longer_reverb_times_have_more_late_tail_energy() {
        fn late_energy(time: f32) -> f32 {
            let mut reverb = Reverb::new(8_000);
            reverb.set_time(time);
            (0..8_000)
                .map(|i| {
                    let input = if i == 0 { 0.1 } else { 0.0 };
                    let (l, r) = reverb.process(input, input);
                    if i >= 4_000 { l * l + r * r } else { 0.0 }
                })
                .sum()
        }

        let short = late_energy(0.2);
        let default = late_energy(2.5);
        let long = late_energy(10.0);
        assert!(
            short < default && default < long,
            "{short}, {default}, {long}"
        );
    }
    #[test]
    fn reverb_is_finite_and_bounded_at_time_extremes() {
        for time in [0.2, 10.0] {
            let mut reverb = Reverb::new(8_000);
            reverb.set_time(time);
            for i in 0..96_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(input, input);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
            }
        }
    }

    #[test]
    fn reverb_pre_delay_shifts_the_wet_onset() {
        fn first_output(pre_delay_ms: u16) -> usize {
            let mut reverb = Reverb::new(8_000);
            reverb.set_pre_delay_ms(pre_delay_ms);
            for _ in 0..64 {
                reverb.process(0.0, 0.0);
            }
            reverb.clear();
            for i in 0..2_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(input, input);
                if l.abs().max(r.abs()) > 0.000_001 {
                    return i;
                }
            }
            panic!("reverb did not produce an output");
        }

        let immediate = first_output(0);
        let delayed = first_output(20);
        assert!((delayed as isize - immediate as isize - 160).abs() <= 1);
    }

    #[test]
    fn brighter_reverb_has_more_late_tail_energy() {
        fn late_energy(tone: f32) -> f32 {
            let mut reverb = Reverb::new(8_000);
            reverb.set_tone(tone);
            (0..8_000)
                .map(|i| {
                    let input = if i == 0 { 0.1 } else { 0.0 };
                    let (l, r) = reverb.process(input, input);
                    if i >= 4_000 { l * l + r * r } else { 0.0 }
                })
                .sum()
        }

        assert!(late_energy(1.0) > late_energy(0.0));
    }

    #[test]
    fn lfo_rate_phase_reset_and_randomness_are_deterministic() {
        use crate::model::{LfoRate, Percent};

        let config = LfoConfig {
            rate: LfoRate::Free {
                rate_percent: Percent::new(100).unwrap(),
            },
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        };
        let mut lfo = Lfo::new(7);
        for _ in 0..50 {
            assert!(lfo.next(Some(config), 120, 1_000.0).is_finite());
        }
        assert!(lfo.phase.min(1.0 - lfo.phase) < 0.0001, "{}", lfo.phase);
        lfo.reset();
        assert_eq!(lfo.phase, 0.0);
        assert_eq!(lfo.value(), 0.0);

        let random = LfoConfig {
            waveform: LfoWaveform::SampleAndHold,
            ..config
        };
        let mut first = Lfo::new(99);
        let mut second = Lfo::new(99);
        for _ in 0..200 {
            assert_eq!(
                first.next(Some(random), 120, 1_000.0),
                second.next(Some(random), 120, 1_000.0)
            );
        }
    }

    #[test]
    fn lfo_start_phase_and_restart_are_exact_and_deterministic() {
        use crate::model::Percent;

        let config = LfoConfig {
            waveform: LfoWaveform::Sine,
            reset_on_trigger: true,
            start_phase: Percent::new(25).unwrap(),
            depth: Percent::new(100).unwrap(),
            ..Default::default()
        };
        let mut lfo = Lfo::new(41);
        assert!((lfo.next(Some(config), 120, 48_000.0) - 1.0).abs() < 0.000_001);
        for _ in 0..100 {
            lfo.next(Some(config), 120, 48_000.0);
        }
        assert!((lfo.restart(config, 120, 48_000.0) - 1.0).abs() < 0.000_001);

        let wrapped = LfoConfig {
            start_phase: Percent::new(100).unwrap(),
            ..config
        };
        assert!(lfo.restart(wrapped, 120, 48_000.0).abs() < 0.000_001);

        let random = LfoConfig {
            waveform: LfoWaveform::SampleAndHold,
            ..config
        };
        let first = lfo.restart(random, 120, 48_000.0);
        for _ in 0..100 {
            lfo.next(Some(random), 120, 48_000.0);
        }
        assert_eq!(lfo.restart(random, 120, 48_000.0), first);
    }

    #[test]
    fn all_lfo_waveforms_remain_bipolar_and_finite() {
        for waveform in LfoWaveform::ALL {
            let config = LfoConfig {
                waveform,
                ..Default::default()
            };
            let mut lfo = Lfo::new(123);
            for _ in 0..48_000 {
                let value = lfo.next(Some(config), 240, 48_000.0);
                assert!(value.is_finite() && (-1.0..=1.0).contains(&value));
            }
        }
    }

    #[test]
    fn pulse_width_extremes_are_band_limited_and_finite() {
        for width in [0.05, 0.5, 0.95] {
            let mut oscillator = PolyBlepOsc::default();
            for _ in 0..48_000 {
                let (_, pulse) = oscillator.next_saw_pulse(440.0, width, 48_000.0);
                assert!(pulse.is_finite() && pulse.abs() <= 2.0);
            }
        }
    }

    #[test]
    fn chord_chorus_modes_are_deterministic_finite_and_stereo() {
        let render = || {
            let mut chorus = StereoChorus::new(48_000);
            chorus.configure(2);
            (0..4_000)
                .map(|sample| chorus.process(if sample == 0 { 1.0 } else { 0.0 }))
                .collect::<Vec<_>>()
        };
        let first = render();
        let second = render();
        assert_eq!(first, second);
        assert!(
            first
                .iter()
                .all(|(left, right)| left.is_finite() && right.is_finite())
        );
        assert!(
            first
                .iter()
                .any(|(left, right)| (left - right).abs() > 0.000_001)
        );
    }

    #[test]
    fn chord_chorus_handles_float_wrap_at_44100_hz() {
        for mode in 1..=2 {
            let mut chorus = StereoChorus::new(44_100);
            chorus.configure(mode);
            for sample in 0..200_000 {
                let (left, right) = chorus.process(if sample % 127 == 0 { 1.0 } else { 0.0 });
                assert!(left.is_finite() && right.is_finite());
            }
        }
    }

    #[test]
    fn chord_chorus_tap_handles_a_near_zero_negative_read() {
        let mut chorus = StereoChorus::new(44_100);
        chorus.pos = 0;
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| chorus.tap(0.00001))).is_ok()
        );
    }

    #[test]
    fn track_effect_chain_bypasses_exactly_and_remains_finite() {
        let mut chain = TrackEffectChain::new(48_000);
        chain.configure(TrackEffects::default(), ParameterLocks::default(), 0);
        for sample in [0.0, 0.1, -0.7, 1.0] {
            assert_eq!(chain.process(sample), (sample, sample));
        }
        assert_eq!(chain.process_stereo(0.25, -0.75), (0.25, -0.75));

        let effects = TrackEffects {
            distortion: crate::model::DistortionParameters {
                drive: crate::model::Percent::new(100).unwrap(),
                tone: crate::model::Percent::new(100).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
            phaser: crate::model::PhaserParameters {
                rate: crate::model::Percent::new(100).unwrap(),
                depth: crate::model::Percent::new(100).unwrap(),
                feedback: crate::model::Percent::new(90).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
            flanger: crate::model::FlangerParameters {
                rate: crate::model::Percent::new(100).unwrap(),
                delay: crate::model::Percent::new(100).unwrap(),
                depth: crate::model::Percent::new(100).unwrap(),
                feedback: crate::model::Percent::new(90).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
            bit_crusher: crate::model::BitCrusherParameters {
                bits: crate::model::Percent::new(100).unwrap(),
                rate: crate::model::Percent::new(100).unwrap(),
                mix: crate::model::Percent::new(100).unwrap(),
            },
        };
        chain.configure(effects, ParameterLocks::default(), 0);
        let mut stereo = false;
        for sample in 0..48_000 {
            let (left, right) = chain.process(if sample == 0 { 1.0 } else { 0.0 });
            assert!(left.is_finite() && right.is_finite());
            stereo |= (left - right).abs() > 0.000_001;
        }
        assert!(stereo);
    }

    #[test]
    fn bit_crusher_mappings_cover_the_documented_range() {
        assert_eq!(bit_crusher_bit_depth(0.0), 16);
        assert_eq!(bit_crusher_bit_depth(50.0), 9);
        assert_eq!(bit_crusher_bit_depth(100.0), 2);
        assert_eq!(bit_crusher_rate_divisor(0.0), 1.0);
        assert!((bit_crusher_rate_divisor(50.0) - 8.0).abs() < 0.000_001);
        assert!((bit_crusher_rate_divisor(100.0) - 64.0).abs() < 0.000_01);
    }

    #[test]
    fn bit_crusher_quantizes_and_holds_stereo_until_the_next_capture() {
        for (rate, held_frames) in [(50, 7), (100, 63)] {
            let mut effects = TrackEffects::default();
            effects.bit_crusher.bits = crate::model::Percent::new(100).unwrap();
            effects.bit_crusher.rate = crate::model::Percent::new(rate).unwrap();
            effects.bit_crusher.mix = crate::model::Percent::new(100).unwrap();
            let mut chain = TrackEffectChain::new(48_000);
            chain.configure(effects, ParameterLocks::default(), 0);

            assert_eq!(chain.process_stereo(0.8, -0.8), (1.0, -1.0));
            for _ in 0..held_frames {
                assert_eq!(chain.process_stereo(0.1, -0.1), (1.0, -1.0));
            }
            assert!(chain.is_active());
            assert_eq!(chain.process_stereo(0.0, 0.0), (0.0, 0.0));
            assert!(!chain.is_active());
        }
        assert_eq!(TrackEffectChain::bit_crush_sample(f32::NAN, 2), 0.0);
    }

    #[test]
    fn disabling_flanger_drains_feedback_without_restoring_wet_mix() {
        let mut chain = TrackEffectChain::new(8_000);
        let mut effects = TrackEffects::default();
        effects.flanger.feedback = crate::model::Percent::new(90).unwrap();
        effects.flanger.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.flanger_l.fill(0.75);
        chain.flanger_r.fill(-0.75);
        chain.flanger_tail_remaining = chain.tail_length();

        chain.configure(TrackEffects::default(), ParameterLocks::default(), 1);
        let output = chain.process_stereo(0.25, -0.5);

        assert_eq!(chain.flanger_mix.value(), 0.0);
        assert!(chain.flanger_active, "feedback should drain internally");
        assert_eq!(output, (0.25, -0.5), "zero mix must remain an exact bypass");
    }

    #[test]
    fn disabling_phaser_drains_feedback_without_restoring_wet_mix() {
        let mut chain = TrackEffectChain::new(8_000);
        let mut effects = TrackEffects::default();
        effects.phaser.feedback = crate::model::Percent::new(90).unwrap();
        effects.phaser.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.phaser_feedback_l = 0.75;
        chain.phaser_feedback_r = -0.5;
        chain.phaser_l[0].state = 0.5;
        chain.phaser_r[0].state = -0.5;
        chain.phaser_tail_remaining = chain.tail_length();

        chain.configure(TrackEffects::default(), ParameterLocks::default(), 1);
        let output = chain.process_stereo(0.25, -0.5);

        assert_eq!(chain.phaser_mix.value(), 0.0);
        assert!(chain.phaser_active, "feedback should drain internally");
        assert_eq!(output, (0.25, -0.5), "zero mix must remain an exact bypass");
    }

    #[test]
    fn maximum_feedback_flanger_tail_is_not_cut_at_a_quarter_second() {
        let sample_rate = 8_000;
        let mut chain = TrackEffectChain::new(sample_rate);
        let mut effects = TrackEffects::default();
        effects.flanger.rate = crate::model::Percent::ZERO;
        effects.flanger.delay = crate::model::Percent::new(100).unwrap();
        effects.flanger.depth = crate::model::Percent::new(100).unwrap();
        effects.flanger.feedback = crate::model::Percent::new(90).unwrap();
        effects.flanger.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.process(1.0);

        let mut late_energy = 0.0;
        for sample in 1..sample_rate / 2 {
            let (left, right) = chain.process(0.0);
            if sample >= sample_rate / 4 {
                late_energy += left * left + right * right;
            }
        }
        assert!(chain.flanger_active);
        assert!(late_energy > 0.000_001, "late tail energy {late_energy}");

        for _ in 0..sample_rate * 2 {
            chain.process(0.0);
        }
        assert!(!chain.flanger_active);
        assert!(chain.flanger_l.iter().all(|sample| *sample == 0.0));
        assert!(chain.flanger_r.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn upstream_distortion_tail_keeps_the_downstream_phaser_awake() {
        let mut chain = TrackEffectChain::new(8_000);
        let mut effects = TrackEffects::default();
        effects.distortion.drive = crate::model::Percent::new(100).unwrap();
        effects.distortion.tone = crate::model::Percent::ZERO;
        effects.distortion.mix = crate::model::Percent::new(100).unwrap();
        effects.phaser.feedback = crate::model::Percent::new(90).unwrap();
        effects.phaser.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.process(1.0);
        chain.clear_phaser();

        chain.process(0.0);

        assert!(chain.distortion_active);
        assert!(chain.phaser_active);
        assert!(chain.phaser_tail_remaining > 0);
    }

    #[test]
    fn phaser_tail_eventually_clears_all_feedback_state() {
        let sample_rate = 8_000;
        let mut chain = TrackEffectChain::new(sample_rate);
        let mut effects = TrackEffects::default();
        effects.phaser.feedback = crate::model::Percent::new(90).unwrap();
        effects.phaser.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        chain.process(1.0);
        for _ in 0..sample_rate * 2 + 1 {
            chain.process(0.0);
        }

        assert!(!chain.phaser_active);
        assert_eq!(chain.phaser_feedback_l, 0.0);
        assert_eq!(chain.phaser_feedback_r, 0.0);
        assert!(
            chain
                .phaser_l
                .iter()
                .chain(chain.phaser_r.iter())
                .all(|stage| stage.state == 0.0)
        );
    }

    #[test]
    fn flanger_delay_is_at_least_one_sample_at_low_sample_rate() {
        let mut chain = TrackEffectChain::new(100);
        let delay = chain.flanger_delay_samples(0.2, 0.0, 0.0);
        assert_eq!(delay, 1.0);

        let last = chain.flanger_l.len() - 1;
        chain.flanger_l[last] = 0.25;
        chain.flanger_l[0] = 4.0;
        assert_eq!(
            TrackEffectChain::read_delay(&chain.flanger_l, 0, delay),
            0.25
        );
    }

    #[test]
    fn flanger_storage_matches_maximum_delay_at_supported_rates() {
        for sample_rate in [8, 44_100, 48_000, 96_000, 192_000] {
            let chain = TrackEffectChain::new(sample_rate);
            let expected = ((sample_rate as f32 * FLANGER_MAX_DELAY_SECONDS).ceil() as usize)
                .saturating_add(2)
                .max(3);
            assert_eq!(chain.flanger_l.len(), expected);
            assert_eq!(chain.flanger_r.len(), expected);
            let maximum = chain.flanger_delay_samples(10.0, 5.0, 0.25);
            assert!(maximum <= (expected - 2) as f32);
            for write in 0..expected {
                assert!(TrackEffectChain::read_delay(&chain.flanger_l, write, maximum).is_finite());
            }
        }
    }

    #[test]
    fn phaser_control_block_tracks_full_rate_coefficients() {
        for sample_rate in [44_100, 48_000, 96_000, 192_000] {
            for (rate, depth) in [(0, 0), (0, 100), (100, 0), (100, 100)] {
                let mut effects = TrackEffects::default();
                effects.phaser.rate = crate::model::Percent::new(rate).unwrap();
                effects.phaser.depth = crate::model::Percent::new(depth).unwrap();
                effects.phaser.mix = crate::model::Percent::new(100).unwrap();
                let mut chain = TrackEffectChain::new(sample_rate);
                chain.configure(effects, ParameterLocks::default(), 0);
                for _ in 0..sample_rate.min(4_096) {
                    chain.process_stereo(0.2, -0.2);
                    let reference_l = TrackEffectChain::phaser_coefficient(
                        chain.phaser_phase,
                        chain.phaser_sweep,
                        chain.sample_rate,
                    );
                    let reference_r = TrackEffectChain::phaser_coefficient(
                        (chain.phaser_phase + 0.5).fract(),
                        chain.phaser_sweep,
                        chain.sample_rate,
                    );
                    assert!(chain.phaser_coefficient_l.is_finite());
                    assert!(chain.phaser_coefficient_r.is_finite());
                    assert!(chain.phaser_coefficient_l.abs() <= 0.980_001);
                    assert!(chain.phaser_coefficient_r.abs() <= 0.980_001);
                    assert!(
                        (chain.phaser_coefficient_l - reference_l).abs() <= 0.005,
                        "sr={sample_rate} rate={rate} depth={depth} phase={} actual={} reference={reference_l}",
                        chain.phaser_phase,
                        chain.phaser_coefficient_l,
                    );
                    assert!(
                        (chain.phaser_coefficient_r - reference_r).abs() <= 0.005,
                        "sr={sample_rate} rate={rate} depth={depth} phase={} actual={} reference={reference_r}",
                        chain.phaser_phase,
                        chain.phaser_coefficient_r,
                    );
                }
            }
        }
    }

    #[test]
    fn phaser_parameter_ramps_remain_smooth_and_stereo_opposed() {
        let mut chain = TrackEffectChain::new(48_000);
        let mut effects = TrackEffects::default();
        effects.phaser.mix = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 0);
        for _ in 0..32 {
            chain.process(0.25);
        }
        effects.phaser.rate = crate::model::Percent::new(100).unwrap();
        effects.phaser.depth = crate::model::Percent::new(100).unwrap();
        chain.configure(effects, ParameterLocks::default(), 256);
        let mut previous = chain.process(0.25);
        let mut stereo = false;
        for _ in 0..512 {
            let output = chain.process(0.25);
            assert!(output.0.is_finite() && output.1.is_finite());
            assert!((output.0 - previous.0).abs() < 0.25);
            assert!((output.1 - previous.1).abs() < 0.25);
            stereo |= (chain.phaser_coefficient_l - chain.phaser_coefficient_r).abs() > 0.000_001;
            previous = output;
        }
        assert!(stereo);
    }

    #[test]
    fn flanger_geometry_constrains_depth_without_clipping_the_cycle() {
        let (center, depth, low, high) = flanger_delay_geometry(0.0, 100.0);
        assert_eq!(center, 0.2);
        assert!((depth - 0.1).abs() < 0.000_001);
        assert!((low - FLANGER_MIN_DELAY_MS).abs() < 0.000_001);
        assert!((high - 0.3).abs() < 0.000_001);

        let (_, full_depth, full_low, full_high) = flanger_delay_geometry(100.0, 100.0);
        assert_eq!(full_depth, 5.0);
        assert_eq!(full_low, 5.0);
        assert_eq!(full_high, 15.0);

        for delay in 0..=100 {
            for depth in 0..=100 {
                let (center, depth, low, high) = flanger_delay_geometry(delay as f32, depth as f32);
                assert!(low >= FLANGER_MIN_DELAY_MS - 0.000_01);
                assert!((center - depth - low).abs() < 0.000_01);
                assert!((center + depth - high).abs() < 0.000_01);
            }
        }
    }

    #[test]
    fn track_effect_chain_is_deterministic() {
        let effects = TrackEffects {
            distortion: crate::model::DistortionParameters {
                drive: crate::model::Percent::new(60).unwrap(),
                tone: crate::model::Percent::new(30).unwrap(),
                mix: crate::model::Percent::new(75).unwrap(),
            },
            phaser: crate::model::PhaserParameters {
                rate: crate::model::Percent::new(25).unwrap(),
                depth: crate::model::Percent::new(50).unwrap(),
                feedback: crate::model::Percent::new(80).unwrap(),
                mix: crate::model::Percent::new(60).unwrap(),
            },
            flanger: crate::model::FlangerParameters {
                rate: crate::model::Percent::new(35).unwrap(),
                delay: crate::model::Percent::new(18).unwrap(),
                depth: crate::model::Percent::new(70).unwrap(),
                feedback: crate::model::Percent::new(65).unwrap(),
                mix: crate::model::Percent::new(55).unwrap(),
            },
            bit_crusher: crate::model::BitCrusherParameters {
                bits: crate::model::Percent::new(70).unwrap(),
                rate: crate::model::Percent::new(45).unwrap(),
                mix: crate::model::Percent::new(50).unwrap(),
            },
        };
        let mut first = TrackEffectChain::new(8_000);
        let mut second = TrackEffectChain::new(8_000);
        first.configure(effects, ParameterLocks::default(), 0);
        second.configure(effects, ParameterLocks::default(), 0);
        for sample in 0..10_000 {
            let input = ((sample as f32) * 0.017).sin() * 0.4;
            assert_eq!(first.process(input), second.process(input));
        }
    }

    #[test]
    fn sidechain_zero_depth_is_unity() {
        let mut compressor = SidechainCompressor::new(48_000);
        let parameters = SidechainParameters {
            depth: crate::model::Percent::ZERO,
            ..SidechainParameters::default()
        };
        compressor.configure(parameters);
        for _ in 0..100 {
            assert_eq!(compressor.process_stereo(1.0, 0.5), 1.0);
        }
    }

    #[test]
    fn sidechain_attack_and_release_follow_peak_envelope() {
        let mut compressor = SidechainCompressor::new(1_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(0).unwrap(),
            release: crate::model::Percent::new(0).unwrap(),
        });
        let first = compressor.process_stereo(1.0, -0.5);
        assert!(compressor.envelope() > 0.0);
        assert!(first < 1.0);
        let ducked = (0..100)
            .map(|_| compressor.process_stereo(1.0, 1.0))
            .last()
            .unwrap();
        assert!(ducked < 0.2);
        let before_release = compressor.envelope();
        for _ in 0..100 {
            compressor.process_stereo(0.0, 0.0);
        }
        assert!(compressor.envelope() < before_release);
        assert!(compressor.current_gain() > ducked);
    }

    #[test]
    fn sidechain_maximum_attenuation_reset_and_finite_output() {
        let mut compressor = SidechainCompressor::new(48_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(100).unwrap(),
            release: crate::model::Percent::new(100).unwrap(),
        });
        for _ in 0..100_000 {
            compressor.process_stereo(2.0, -2.0);
        }
        assert!((compressor.current_gain() - 10.0_f32.powf(-18.0 / 20.0)).abs() < 0.0001);
        assert!(
            compressor
                .process_stereo(f32::NAN, f32::INFINITY)
                .is_finite()
        );
        compressor.reset();
        assert_eq!(compressor.envelope(), 0.0);
        assert_eq!(compressor.current_gain(), 10.0_f32.powf(-0.0));
    }

    #[test]
    fn sidechain_recovers_after_nonfinite_detector_input() {
        let mut compressor = SidechainCompressor::new(48_000);
        compressor.configure(SidechainParameters {
            depth: crate::model::Percent::new(100).unwrap(),
            attack: crate::model::Percent::new(0).unwrap(),
            ..SidechainParameters::default()
        });
        assert_eq!(compressor.process_stereo(f32::NAN, f32::NAN), 1.0);
        assert_eq!(compressor.envelope(), 0.0);
        for _ in 0..1_000 {
            compressor.process_stereo(1.0, 1.0);
        }
        assert!(compressor.envelope() > 0.99);
        assert!(compressor.current_gain() < 0.13);
    }

    #[test]
    fn bass_vca_holds_after_its_independent_filter_contour_has_decayed() {
        let mut vca = BassVcaEnvelope::new(8_000.0);
        let mut contour = BassFilterEnvelope::new(8_000.0);
        vca.gate_on();
        contour.trigger(0.0);
        for _ in 0..1_600 {
            vca.next_sample();
            contour.next_sample();
        }
        assert!(vca.value() > 0.99, "a held Bass gate must remain audible");
        assert!(
            contour.value() < 0.001,
            "minimum decay should only close the filter contour"
        );
    }

    #[test]
    fn bass_vca_releases_with_fixed_timing() {
        let mut vca = BassVcaEnvelope::new(8_000.0);
        vca.gate_on();
        for _ in 0..80 {
            vca.next_sample();
        }
        vca.gate_off();
        for _ in 0..500 {
            vca.next_sample();
        }
        assert!(vca.is_idle());
    }

    #[test]
    fn bass_accent_retriggers_smoothly_then_decays() {
        let mut accent = BassAccentEnvelope::new(8_000.0);
        accent.trigger(true);
        let first = accent.next_sample();
        assert!(first > 0.0 && first < 1.0);
        for _ in 0..40 {
            accent.next_sample();
        }
        let peak = accent.value();
        assert!(peak > 0.7);
        for _ in 0..160 {
            accent.next_sample();
        }
        let decaying = accent.value();
        assert!(decaying > 0.1 && decaying < peak);

        accent.trigger(true);
        let retriggered = accent.next_sample();
        assert!(retriggered > decaying && retriggered < 1.0);
        for _ in 0..1_600 {
            accent.next_sample();
        }
        assert!(accent.value() < 0.001, "accent contour must not sustain");
    }

    #[test]
    fn bass_filter_is_finite_at_extreme_parameters_and_sample_rates() {
        for sample_rate in [8_000.0, 44_100.0, 96_000.0] {
            for cutoff in [20.0, sample_rate * 0.42] {
                for resonance in [0.0, 1.0] {
                    let mut filter = BassFilter::new();
                    filter.set_parameters_smoothed(cutoff, resonance, sample_rate * 2.0, 0);
                    for sample in 0..20_000 {
                        let input = ((sample as f32) * 0.071).sin() * 2.0;
                        assert!(filter.process(input).is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn bass_filter_approaches_four_pole_far_stopband_rolloff() {
        fn amplitude_at(frequency: f32) -> f32 {
            let sample_rate = 96_000.0;
            let mut filter = BassFilter::new();
            filter.set_parameters_smoothed(250.0, 0.0, sample_rate, 0);
            let mut sine = 0.0;
            let mut cosine = 0.0;
            let mut count = 0;
            for sample in 0..96_000 {
                let phase = std::f32::consts::TAU * frequency * sample as f32 / sample_rate;
                let input = 0.1 * phase.sin();
                let output = filter.process(input);
                if sample >= 48_000 {
                    sine += output * phase.sin();
                    cosine += output * phase.cos();
                    count += 1;
                }
            }
            2.0 * (sine * sine + cosine * cosine).sqrt() / count as f32
        }

        let lower_octave = amplitude_at(2_000.0);
        let upper_octave = amplitude_at(4_000.0);
        let attenuation_db = 20.0 * (upper_octave / lower_octave).log10();
        assert!(
            (-30.0..=-18.0).contains(&attenuation_db),
            "expected four-pole far-stopband attenuation, got {attenuation_db:.1} dB/octave"
        );
    }

    #[test]
    fn bass_filter_resonance_emphasizes_cutoff_relative_to_the_low_end() {
        fn rms_at(resonance: f32, frequency: f32) -> f32 {
            let sample_rate = 48_000.0;
            let mut filter = BassFilter::new();
            filter.set_parameters_smoothed(250.0, resonance, sample_rate, 0);
            let mut energy = 0.0;
            let mut count = 0;
            for sample in 0..48_000 {
                let input = (std::f32::consts::TAU * frequency * sample as f32 / sample_rate).sin();
                let output = filter.process(input);
                if sample > 24_000 {
                    energy += output * output;
                    count += 1;
                }
            }
            (energy / count as f32).sqrt()
        }

        let low_without_resonance = rms_at(0.0, 80.0);
        let low_with_resonance = rms_at(1.0, 80.0);
        let cutoff_without_resonance = rms_at(0.0, 250.0);
        let cutoff_with_resonance = rms_at(1.0, 250.0);
        let ratio_without_resonance = cutoff_without_resonance / low_without_resonance;
        let ratio_with_resonance = cutoff_with_resonance / low_with_resonance;
        assert!(
            ratio_with_resonance > ratio_without_resonance,
            "resonance did not emphasize cutoff relative to the low end: \
             {ratio_without_resonance:.3} -> {ratio_with_resonance:.3}"
        );
    }
}
