//! Deterministic, fill-only pattern idea generation.
//!
//! This module deliberately works on the editor's active step cache.  It has
//! no UI or transport dependencies, and generated values are ordinary model
//! events, so projects remain fully compatible with the existing JSON format.

use crate::model::{
    ArpeggioConfig, ChordShape, ParameterLocks, Percent, Project, Step, StepEvent, TrackKind,
    TriggerCondition,
};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    WholePattern,
    Track(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChordShapePool {
    Default,
    RootShapes,
    #[default]
    AllShapes,
}

impl ChordShapePool {
    pub const ALL: [Self; 3] = [Self::Default, Self::RootShapes, Self::AllShapes];
}

impl fmt::Display for ChordShapePool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Default => "Default",
                Self::RootShapes => "Root shapes",
                Self::AllShapes => "All shapes",
            }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub target: Target,
    pub seed: u64,
    pub density: Percent,
    pub range_low: u8,
    pub range_high: u8,
    pub chord_shapes: ChordShapePool,
    pub ties: Percent,
    pub accents: Percent,
    pub slides: Percent,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target: Target::WholePattern,
            seed: 0x0054_4752_4f4f_5645,
            density: Percent::new(48).unwrap(),
            range_low: 2,
            range_high: 6,
            chord_shapes: ChordShapePool::default(),
            ties: Percent::new(18).unwrap(),
            accents: Percent::new(24).unwrap(),
            slides: Percent::new(18).unwrap(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub tracks: Vec<Vec<Step>>,
    pub inserted: usize,
}

const MAX_OCTAVE: u8 = 7;

fn normalized_octave_range(config: Config) -> (u8, u8) {
    let low = config.range_low.min(MAX_OCTAVE);
    let high = config.range_high.min(MAX_OCTAVE).max(low);
    (low, high)
}

fn is_anchor(kind: TrackKind, step: usize, groove: Option<&[bool]>) -> bool {
    match kind {
        TrackKind::Kick => step % 16 == 0 || step % 16 == 7,
        TrackKind::Snare => step % 16 == 4 || step % 16 == 12,
        TrackKind::Hat => step % 2 == 0,
        TrackKind::Tom => step % 16 == 10 || step % 16 == 14,
        TrackKind::Cymbal => step % 16 == 0,
        TrackKind::Rimshot => step % 16 == 4 || step % 16 == 12,
        TrackKind::Bass => {
            groove.is_some_and(|g| g.get(step).copied().unwrap_or(false)) || step % 4 == 0
        }
        TrackKind::Chord => step % 8 == 0,
        TrackKind::Lead => step % 4 == 2,
    }
}

#[derive(Clone, Copy)]
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = if x == 0 { 0x9e37_79b9_7f4a_7c15 } else { x };
        self.0
    }
    fn percent(&mut self, value: Percent) -> bool {
        (self.next() % 100) < u64::from(value.get())
    }
    fn range(&mut self, low: u8, high: u8) -> u8 {
        if low >= high {
            low
        } else {
            low + (self.next() % u64::from(high - low + 1)) as u8
        }
    }
}

#[cfg(test)]
fn generate(project: &Project, config: Config) -> Generated {
    generate_for_pattern(project, 0, config)
}

pub fn generate_for_pattern(project: &Project, pattern: usize, config: Config) -> Generated {
    let mut rng = Rng(config.seed);
    let (range_low, range_high) = normalized_octave_range(config);
    let mut tracks: Vec<Vec<Step>> = project.patterns[pattern]
        .tracks
        .iter()
        .map(|track| track.steps.clone())
        .collect();
    let mut inserted = 0;
    let selected = |index: usize| {
        matches!(config.target, Target::WholePattern) || config.target == Target::Track(index)
    };

    // Drum anchors are made first so the pitched tracks can follow the groove.
    for (index, track) in project.tracks.iter().enumerate() {
        if selected(index)
            && matches!(
                track.kind,
                TrackKind::Kick
                    | TrackKind::Snare
                    | TrackKind::Hat
                    | TrackKind::Tom
                    | TrackKind::Cymbal
                    | TrackKind::Rimshot
            )
        {
            inserted += fill_track(
                &mut tracks[index],
                track.kind,
                &mut rng,
                config.density,
                config.accents,
                config.slides,
                config.chord_shapes,
                range_low,
                range_high,
                None,
            );
        }
    }
    let groove: Vec<bool> = tracks
        .first()
        .map(|steps| {
            steps
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    e.is_some()
                        || tracks
                            .get(2)
                            .and_then(|h| h.get(i))
                            .is_some_and(Option::is_some)
                })
                .collect()
        })
        .unwrap_or_default();
    for (index, track) in project.tracks.iter().enumerate() {
        if !selected(index)
            || matches!(
                track.kind,
                TrackKind::Kick
                    | TrackKind::Snare
                    | TrackKind::Hat
                    | TrackKind::Tom
                    | TrackKind::Cymbal
                    | TrackKind::Rimshot
            )
        {
            continue;
        }
        let rhythm = if track.kind == TrackKind::Bass {
            Some(groove.as_slice())
        } else {
            None
        };
        inserted += fill_track(
            &mut tracks[index],
            track.kind,
            &mut rng,
            config.density,
            config.accents,
            config.slides,
            config.chord_shapes,
            range_low,
            range_high,
            rhythm,
        );
        if matches!(
            track.kind,
            TrackKind::Bass | TrackKind::Chord | TrackKind::Lead
        ) {
            inserted += add_ties(&mut tracks[index], &mut rng, config.ties);
        }
    }
    Generated { tracks, inserted }
}

#[allow(clippy::too_many_arguments)]
fn fill_track(
    steps: &mut [Step],
    kind: TrackKind,
    rng: &mut Rng,
    density: Percent,
    accents: Percent,
    slides: Percent,
    chord_shapes: ChordShapePool,
    range_low: u8,
    range_high: u8,
    groove: Option<&[bool]>,
) -> usize {
    let mut count = 0;
    for (i, slot) in steps.iter_mut().enumerate() {
        if slot.is_some() {
            continue;
        }
        let anchor = is_anchor(kind, i, groove);
        let probability = if anchor {
            density.saturating_add(22)
        } else {
            density
        };
        if !rng.percent(probability) {
            continue;
        }
        let accent = anchor && rng.percent(accents);
        *slot = Some(match kind {
            TrackKind::Kick
            | TrackKind::Snare
            | TrackKind::Hat
            | TrackKind::Tom
            | TrackKind::Cymbal
            | TrackKind::Rimshot => StepEvent::Trigger {
                accent,
                recipe: crate::model::DrumRecipeSlot::ONE,
                condition: TriggerCondition::Always,
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: ParameterLocks::default(),
            },
            TrackKind::Bass => StepEvent::BassNote {
                degree: rng.range(1, 5),
                octave: rng.range(range_low, range_high),
                accent,
                slide: rng.percent(slides),
                condition: TriggerCondition::Always,
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: ParameterLocks::default(),
            },
            TrackKind::Chord => StepEvent::Note {
                degree: rng.range(1, 7),
                octave: rng.range(range_low, range_high),
                accent,
                chord_shape: random_chord_shape(rng, chord_shapes),
                arpeggio: ArpeggioConfig::default(),
                condition: TriggerCondition::Always,
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: ParameterLocks::default(),
            },
            TrackKind::Lead => StepEvent::LeadNote {
                degree: rng.range(1, 8),
                octave: rng.range(range_low, range_high),
                accent,
                slide: rng.percent(slides),
                condition: TriggerCondition::Always,
                retrigger_count: 1,
                microtiming: crate::model::Microtiming::ZERO,
                locks: ParameterLocks::default(),
            },
        });
        count += 1;
    }
    count
}

const ROOT_CHORD_SHAPES: [ChordShape; 8] = [
    ChordShape::Single,
    ChordShape::DyadThird,
    ChordShape::DyadFifth,
    ChordShape::TriadRoot,
    ChordShape::SeventhRoot,
    ChordShape::SixthRoot,
    ChordShape::Sus2Root,
    ChordShape::Sus4Root,
];

fn random_chord_shape(rng: &mut Rng, pool: ChordShapePool) -> Option<ChordShape> {
    let shapes: &[ChordShape] = match pool {
        ChordShapePool::Default => return None,
        ChordShapePool::RootShapes => &ROOT_CHORD_SHAPES,
        ChordShapePool::AllShapes => &ChordShape::ALL,
    };
    let shape = shapes[usize::from(rng.range(0, shapes.len() as u8 - 1))];
    (shape != ChordShape::default()).then_some(shape)
}

fn add_ties(steps: &mut [Step], rng: &mut Rng, amount: Percent) -> usize {
    let mut count = 0;
    for i in 1..steps.len() {
        if steps[i].is_none()
            && rng.percent(amount)
            && matches!(
                steps[i - 1],
                Some(
                    StepEvent::BassNote { .. }
                        | StepEvent::Note { .. }
                        | StepEvent::LeadNote { .. }
                )
            )
        {
            steps[i] = Some(StepEvent::Tie {
                locks: ParameterLocks::default(),
            });
            count += 1;
        }
    }
    // A generated sequence may contain no notes if density is zero.  In that
    // case no ties were inserted, and existing all-tie input is left intact.
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, SYNTH_TRACK_START, StepEvent};

    #[test]
    fn same_seed_is_repeatable_and_different_seed_changes_ideas() {
        let p = Project::new();
        let a = generate(&p, Config::default());
        let b = generate(&p, Config::default());
        let mut c = Config {
            seed: 99,
            ..Config::default()
        };
        c.density = Percent::new(80).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, generate(&p, c));
    }

    #[test]
    fn rimshot_anchors_the_two_backbeats() {
        let anchors = (0..16)
            .filter(|step| is_anchor(TrackKind::Rimshot, *step, None))
            .collect::<Vec<_>>();
        assert_eq!(anchors, vec![4, 12]);
    }

    #[test]
    fn generated_ties_have_note_sources() {
        let p = Project::new();
        let c = Config {
            density: Percent::new(100).unwrap(),
            ties: Percent::new(100).unwrap(),
            ..Config::default()
        };
        let result = generate(&p, c);
        for steps in result.tracks {
            for (i, event) in steps.iter().enumerate() {
                if matches!(event, Some(StepEvent::Tie { .. })) {
                    assert!(
                        i > 0
                            && matches!(
                                steps[i - 1],
                                Some(
                                    StepEvent::BassNote { .. }
                                        | StepEvent::Note { .. }
                                        | StepEvent::LeadNote { .. }
                                )
                            )
                    );
                }
            }
        }
    }

    #[test]
    fn selected_track_does_not_change_other_tracks() {
        let p = Project::new();
        let result = generate(
            &p,
            Config {
                target: Target::Track(SYNTH_TRACK_START),
                ..Config::default()
            },
        );
        assert!(
            result.tracks[..SYNTH_TRACK_START]
                .iter()
                .all(|s| s.iter().all(Option::is_none))
        );
        assert!(
            result.tracks[SYNTH_TRACK_START + 1..]
                .iter()
                .all(|s| s.iter().all(Option::is_none))
        );
    }

    fn generated_octaves(steps: &[Step]) -> Vec<u8> {
        steps
            .iter()
            .filter_map(|event| match event {
                Some(
                    StepEvent::BassNote { octave, .. }
                    | StepEvent::Note { octave, .. }
                    | StepEvent::LeadNote { octave, .. },
                ) => Some(*octave),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn custom_octave_bounds_are_inclusive_for_pitched_tracks() {
        let p = Project::new();
        let result = generate(
            &p,
            Config {
                density: Percent::new(100).unwrap(),
                range_low: 4,
                range_high: 5,
                ties: Percent::ZERO,
                ..Config::default()
            },
        );

        for track in [
            crate::model::SYNTH_TRACK_START,
            crate::model::CHORD_TRACK_INDEX,
            crate::model::LEAD_TRACK_INDEX,
        ] {
            let octaves = generated_octaves(&result.tracks[track]);
            assert!(!octaves.is_empty());
            assert!(octaves.iter().all(|octave| (4..=5).contains(octave)));
        }
    }

    #[test]
    fn collapsed_octave_range_generates_only_that_octave() {
        let p = Project::new();
        let result = generate(
            &p,
            Config {
                density: Percent::new(100).unwrap(),
                range_low: 3,
                range_high: 3,
                ties: Percent::ZERO,
                ..Config::default()
            },
        );

        for track in [
            crate::model::SYNTH_TRACK_START,
            crate::model::CHORD_TRACK_INDEX,
            crate::model::LEAD_TRACK_INDEX,
        ] {
            assert!(
                generated_octaves(&result.tracks[track])
                    .iter()
                    .all(|octave| *octave == 3)
            );
        }
    }

    #[test]
    fn chord_roots_are_randomized_within_the_configured_range() {
        let mut p = Project::new();
        p.patterns[0].tracks[crate::model::CHORD_TRACK_INDEX]
            .steps
            .resize(64, None);
        let result = generate(
            &p,
            Config {
                target: Target::Track(crate::model::CHORD_TRACK_INDEX),
                density: Percent::new(100).unwrap(),
                range_low: 2,
                range_high: 6,
                ties: Percent::ZERO,
                ..Config::default()
            },
        );
        let octaves = generated_octaves(&result.tracks[crate::model::CHORD_TRACK_INDEX]);
        assert!(octaves.iter().all(|octave| (2..=6).contains(octave)));
        assert!(octaves.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn generated_slides_follow_the_configured_probability_on_bass_and_lead() {
        let p = Project::new();
        for track in [
            crate::model::SYNTH_TRACK_START,
            crate::model::LEAD_TRACK_INDEX,
        ] {
            let without_slides = generate(
                &p,
                Config {
                    target: Target::Track(track),
                    density: Percent::new(100).unwrap(),
                    ties: Percent::ZERO,
                    slides: Percent::ZERO,
                    ..Config::default()
                },
            );
            assert!(without_slides.tracks[track].iter().all(|event| matches!(
                event,
                Some(
                    StepEvent::BassNote { slide: false, .. }
                        | StepEvent::LeadNote { slide: false, .. }
                )
            )));

            let all_slides = generate(
                &p,
                Config {
                    target: Target::Track(track),
                    density: Percent::new(100).unwrap(),
                    ties: Percent::ZERO,
                    slides: Percent::new(100).unwrap(),
                    ..Config::default()
                },
            );
            assert!(all_slides.tracks[track].iter().all(|event| matches!(
                event,
                Some(
                    StepEvent::BassNote { slide: true, .. }
                        | StepEvent::LeadNote { slide: true, .. }
                )
            )));
        }
    }

    #[test]
    fn chord_shape_pools_generate_only_their_documented_shapes() {
        let mut p = Project::new();
        let track = crate::model::CHORD_TRACK_INDEX;
        p.patterns[0].tracks[track].steps.resize(64, None);
        let shapes_for = |chord_shapes| {
            generate(
                &p,
                Config {
                    target: Target::Track(track),
                    density: Percent::new(100).unwrap(),
                    ties: Percent::ZERO,
                    chord_shapes,
                    ..Config::default()
                },
            )
            .tracks[track]
                .iter()
                .filter_map(|event| match event {
                    Some(StepEvent::Note { chord_shape, .. }) => {
                        Some(chord_shape.unwrap_or_default())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert!(
            shapes_for(ChordShapePool::Default)
                .iter()
                .all(|shape| *shape == ChordShape::TriadRoot)
        );
        let root_shapes = shapes_for(ChordShapePool::RootShapes);
        assert!(
            root_shapes
                .iter()
                .all(|shape| ROOT_CHORD_SHAPES.contains(shape))
        );
        assert!(root_shapes.windows(2).any(|pair| pair[0] != pair[1]));

        let all_shapes = shapes_for(ChordShapePool::AllShapes);
        assert!(
            all_shapes
                .iter()
                .all(|shape| ChordShape::ALL.contains(shape))
        );
        assert!(
            all_shapes
                .iter()
                .any(|shape| !ROOT_CHORD_SHAPES.contains(shape))
        );
    }

    #[test]
    fn generated_pattern_is_valid_and_uses_lead_note_events() {
        let mut project = Project::new();
        let generated = generate(
            &project,
            Config {
                density: Percent::new(100).unwrap(),
                ties: Percent::new(100).unwrap(),
                ..Config::default()
            },
        );

        for (track, steps) in project.patterns[0].tracks.iter_mut().zip(generated.tracks) {
            track.steps = steps;
        }
        project.validate().unwrap();
        assert!(
            project.patterns[0].tracks[crate::model::LEAD_TRACK_INDEX]
                .steps
                .iter()
                .any(|event| matches!(event, Some(StepEvent::LeadNote { .. })))
        );
    }
}
