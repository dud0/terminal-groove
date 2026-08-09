use crate::model::Project;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum ProjectIoError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid project JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid project in {path}: {source}")]
    Validation {
        path: PathBuf,
        #[source]
        source: crate::model::ValidationError,
    },
    #[error("could not save {path}: {source}")]
    Save {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub fn load(path: &Path) -> Result<Project, ProjectIoError> {
    let bytes = fs::read(path).map_err(|source| ProjectIoError::Read {
        path: path.into(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| ProjectIoError::Json {
            path: path.into(),
            source,
        })?;
    let version = value.get("format_version").and_then(|value| value.as_u64());
    if version != Some(17) {
        return Err(ProjectIoError::Validation {
            path: path.into(),
            source: crate::model::ValidationError::Version(version.unwrap_or_default() as u32),
        });
    }
    let mut project: Project =
        serde_json::from_value(value).map_err(|source| ProjectIoError::Json {
            path: path.into(),
            source,
        })?;
    // Format 17 briefly accepted these assignments even though the audio
    // renderer never applied them. Preserve project compatibility while
    // removing only the two inert assignments before strict validation.
    for track in &mut project.tracks {
        track.lfos.noise = None;
        track.lfos.keyboard_tracking = None;
    }
    project
        .validate()
        .map_err(|source| ProjectIoError::Validation {
            path: path.into(),
            source,
        })?;
    if !project.activate_pattern(0) {
        return Err(ProjectIoError::Validation {
            path: path.into(),
            source: crate::model::ValidationError::TrackCount,
        });
    }
    Ok(project)
}

pub fn save_atomic(path: &Path, project: &Project) -> Result<(), ProjectIoError> {
    project
        .validate()
        .map_err(|source| ProjectIoError::Validation {
            path: path.into(),
            source,
        })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(parent)?;
        let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut out = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut out, project).map_err(io::Error::other)?;
        out.write_all(b"\n")?;
        out.flush()?;
        out.get_ref().sync_all()?;
        fs::rename(&tmp, path)?;
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|source| ProjectIoError::Save {
        path: path.into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_newline() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.groove.json");
        let p = Project::new();
        save_atomic(&f, &p).unwrap();
        assert_eq!(load(&f).unwrap(), p);
        assert!(fs::read(&f).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn save_creates_missing_parent_directory() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".projects").join("project.groove.json");

        save_atomic(&f, &Project::new()).unwrap();

        assert!(f.is_file());
        assert_eq!(load(&f).unwrap(), Project::new());
    }

    #[test]
    fn dynamic_pattern_counts_round_trip_and_bounds_are_strict() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("patterns.groove.json");
        for count in [1, 2, 100] {
            let mut value = serde_json::to_value(Project::new()).unwrap();
            let pattern = value["patterns"][0].clone();
            value["patterns"] = serde_json::Value::Array(vec![pattern; count]);
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(load(&path).unwrap().patterns.len(), count);
        }
        for count in [0, 101] {
            let mut value = serde_json::to_value(Project::new()).unwrap();
            let pattern = value["patterns"][0].clone();
            value["patterns"] = if count == 0 {
                serde_json::Value::Array(Vec::new())
            } else {
                serde_json::Value::Array(vec![pattern; count])
            };
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(matches!(
                load(&path),
                Err(ProjectIoError::Validation {
                    source: crate::model::ValidationError::PatternCount(_),
                    ..
                })
            ));
        }
    }

    #[test]
    fn saving_selected_nonzero_pattern_preserves_every_canonical_pattern() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selected-pattern.groove.json");
        let mut project = Project::new();
        project.patterns.push(project.patterns[0].clone());
        project.patterns[0].tracks[0].steps[0] = Some(crate::model::StepEvent::Trigger {
            accent: false,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.patterns[1].tracks[0].steps[1] = Some(crate::model::StepEvent::Trigger {
            accent: true,
            condition: Default::default(),
            retrigger_count: 1,
            locks: Default::default(),
        });
        project.activate_pattern(1);

        save_atomic(&path, &project).unwrap();
        let loaded = load(&path).unwrap();

        assert!(loaded.patterns[0].tracks[0].steps[0].is_some());
        assert!(loaded.patterns[1].tracks[0].steps[1].is_some());
        assert!(loaded.patterns[0].tracks[0].steps[1].is_none());
    }

    #[test]
    fn malformed_pattern_tracks_are_rejected_without_recursive_validation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("malformed-pattern.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["patterns"][0]["tracks"][0]["steps"] = serde_json::json!([null]);
        value["patterns"][0]["tracks"].as_array_mut().unwrap().pop();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        assert!(matches!(
            load(&path),
            Err(ProjectIoError::Validation {
                source: crate::model::ValidationError::TrackCount,
                ..
            })
        ));
    }
    #[test]
    fn reject_unsupported_schema() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(
            &f,
            r#"{"format_version":14,"globals":{},"tracks":[],"wat":1}"#,
        )
        .unwrap();
        assert!(matches!(
            load(&f),
            Err(ProjectIoError::Validation {
                source: crate::model::ValidationError::Version(14),
                ..
            })
        ));
    }
    #[test]
    fn default_schema_uses_required_names() {
        let value = serde_json::to_value(Project::new()).unwrap();
        assert_eq!(value["format_version"], 17);
        assert_eq!(value["globals"]["key"], "C");
        assert_eq!(value["globals"]["delay_division"], "eighth");
        assert_eq!(value["globals"]["reverb_tone"], 40);
        assert_eq!(value["globals"]["reverb_pre_delay_ms"], 20);
        assert_eq!(value["globals"]["reverb_return"], 30);
        assert_eq!(value["tracks"].as_array().unwrap().len(), 8);
        assert_eq!(value["tracks"][0]["name"], "Kick");
        assert_eq!(value["tracks"][4]["kind"], "cymbal");
        assert_eq!(value["tracks"][4]["name"], "Cymbal");
        assert_eq!(value["tracks"][3]["kind"], "tom");
        assert_eq!(value["tracks"][3]["instrument"]["tune"], 50);
        assert_eq!(value["tracks"][3]["instrument"]["tone"], 50);
        assert_eq!(value["tracks"][3]["instrument"]["decay"], 40);
        assert_eq!(value["tracks"][4]["instrument"]["tone"], 55);
        assert_eq!(value["tracks"][6]["kind"], "chord");
        assert_eq!(value["tracks"][6]["name"], "Chord");
        assert_eq!(value["tracks"][6]["reverb_send"], 20);
        assert_eq!(value["tracks"][6]["instrument"]["chorus"], "i");
        assert_eq!(value["tracks"][6]["instrument"]["sub_oscillator"], 0);
        assert_eq!(value["tracks"][0]["effects"]["flanger"]["rate"], 25);
        assert_eq!(value["tracks"][0]["effects"]["flanger"]["delay"], 18);
        assert_eq!(value["tracks"][7]["kind"], "lead");
        assert_eq!(value["tracks"][7]["name"], "Lead");
        assert_eq!(value["tracks"][7]["reverb_send"], 20);
        assert_eq!(value["tracks"][0]["lfos"], serde_json::json!({}));
        assert!(value["tracks"][0].get("input_degree").is_none());
        assert_eq!(
            value["patterns"].as_array().unwrap().len(),
            crate::model::MIN_PATTERN_COUNT
        );
    }

    #[test]
    fn current_schema_without_optional_reverb_controls_uses_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("reverb_tone");
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("reverb_pre_delay_ms");
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("reverb_return");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.globals.reverb_tone.get(), 40);
        assert_eq!(loaded.globals.reverb_pre_delay_ms, 20);
        assert_eq!(loaded.globals.reverb_return.get(), 30);
    }

    #[test]
    fn sidechain_is_required_and_uses_bounded_percentages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sidechain.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("sidechain");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["globals"]["sidechain"]["release"] = 101.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        let mut project = Project::new();
        project.globals.sidechain.depth = crate::model::Percent::new(80).unwrap();
        save_atomic(&path, &project).unwrap();
        assert_eq!(load(&path).unwrap(), project);
    }

    #[test]
    fn missing_track_probability_loads_as_100_percent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probability.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        for track in value["tracks"].as_array_mut().unwrap() {
            track.as_object_mut().unwrap().remove("probability");
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load(&path).unwrap();
        assert!(
            loaded
                .tracks
                .iter()
                .all(|track| track.probability == crate::model::Percent::new(100).unwrap())
        );
    }

    #[test]
    fn track_probability_round_trips_and_rejects_invalid_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probability.groove.json");
        let mut project = Project::new();
        project.tracks[crate::model::SYNTH_TRACK_START].probability =
            crate::model::Percent::new(37).unwrap();
        save_atomic(&path, &project).unwrap();
        assert_eq!(load(&path).unwrap(), project);

        let mut value = serde_json::to_value(&project).unwrap();
        value["tracks"][3]["probability"] = 101.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));
    }

    #[test]
    fn mixed_track_lengths_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("mixed.groove.json");
        let mut project = Project::new();
        for (track, length) in project.tracks.iter_mut().zip([1, 7, 16, 31, 32, 64]) {
            track.steps.resize(length, None);
        }
        save_atomic(&f, &project).unwrap();
        assert_eq!(load(&f).unwrap(), project);
    }

    #[test]
    fn legacy_projects_are_rejected_after_the_format_bump() {
        for json in [
            r#"{"format_version":15}"#,
            r#"{"format_version":15,"globals":{},"tracks":[]}"#,
        ] {
            let path = tempfile::NamedTempFile::new().unwrap();
            fs::write(path.path(), json).unwrap();
            assert!(matches!(
                load(path.path()),
                Err(ProjectIoError::Validation {
                    source: crate::model::ValidationError::Version(15),
                    ..
                })
            ));
        }
    }

    #[test]
    fn lfo_schema_round_trips_synced_and_free_rates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lfos.groove.json");
        let mut project = Project::new();
        project.tracks[0].lfos.tune = Some(crate::model::LfoConfig {
            waveform: crate::model::LfoWaveform::SampleAndHold,
            rate: crate::model::LfoRate::Free {
                rate_percent: crate::model::Percent::new(75).unwrap(),
            },
            depth: crate::model::Percent::new(40).unwrap(),
            enabled: true,
        });
        project.tracks[crate::model::SYNTH_TRACK_START].lfos.cutoff =
            Some(crate::model::LfoConfig::default());
        project.tracks[crate::model::CHORD_TRACK_INDEX].lfos.pitch =
            Some(crate::model::LfoConfig {
                depth: crate::model::Percent::new(100).unwrap(),
                ..Default::default()
            });
        project.tracks[crate::model::LEAD_TRACK_INDEX].lfos.pitch =
            Some(crate::model::LfoConfig::default());
        save_atomic(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, project);
        let json = fs::read_to_string(path).unwrap();
        assert!(json.contains("sample_and_hold"));
        assert!(json.contains("rate_percent"));
        assert!(json.contains("quarter"));
        assert!(json.contains("\"pitch\""));
    }

    #[test]
    fn loading_strips_only_legacy_noise_and_keyboard_tracking_lfos() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-inert-lfos.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        let assignment = serde_json::to_value(crate::model::LfoConfig::default()).unwrap();
        value["tracks"][crate::model::CHORD_TRACK_INDEX]["lfos"]["noise"] = assignment.clone();
        value["tracks"][crate::model::LEAD_TRACK_INDEX]["lfos"]["keyboard_tracking"] =
            assignment.clone();
        value["tracks"][crate::model::LEAD_TRACK_INDEX]["lfos"]["cutoff"] = assignment;
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load(&path).unwrap();
        assert!(
            loaded.tracks[crate::model::CHORD_TRACK_INDEX]
                .lfos
                .noise
                .is_none()
        );
        assert!(
            loaded.tracks[crate::model::LEAD_TRACK_INDEX]
                .lfos
                .keyboard_tracking
                .is_none()
        );
        assert!(
            loaded.tracks[crate::model::LEAD_TRACK_INDEX]
                .lfos
                .cutoff
                .is_some()
        );

        save_atomic(&path, &loaded).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            saved["tracks"][crate::model::CHORD_TRACK_INDEX]["lfos"]
                .get("noise")
                .is_none()
        );
        assert!(
            saved["tracks"][crate::model::LEAD_TRACK_INDEX]["lfos"]
                .get("keyboard_tracking")
                .is_none()
        );
        assert!(
            saved["tracks"][crate::model::LEAD_TRACK_INDEX]["lfos"]
                .get("cutoff")
                .is_some()
        );
    }

    #[test]
    fn lfo_schema_rejects_unknown_and_missing_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-lfo.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][0]["lfos"]["wat"] =
            serde_json::to_value(crate::model::LfoConfig::default()).unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][0].as_object_mut().unwrap().remove("lfos");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));
    }

    #[test]
    fn pitch_lfo_is_rejected_on_non_pitched_tracks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-pitch-lfo.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][3]["lfos"]["pitch"] =
            serde_json::to_value(crate::model::LfoConfig::default()).unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            load(&path),
            Err(ProjectIoError::Validation {
                source: crate::model::ValidationError::Lfo(3, "pitch"),
                ..
            })
        ));
    }

    #[test]
    fn chord_shape_schema_round_trips_and_defaults_legacy_notes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chords.groove.json");
        let mut project = Project::new();
        project.tracks[crate::model::CHORD_TRACK_INDEX].input_chord_shape =
            Some(crate::model::ChordShape::SeventhRoot);
        project.patterns[0].tracks[crate::model::CHORD_TRACK_INDEX].steps[0] =
            Some(crate::model::StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: Some(crate::model::ChordShape::SeventhFirstInversion),
                arpeggio: crate::model::ArpeggioConfig::default(),
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
        save_atomic(&path, &project).unwrap();
        assert_eq!(load(&path).unwrap(), project);

        let mut value = serde_json::to_value(load(&path).unwrap()).unwrap();
        value["patterns"][0]["tracks"][crate::model::CHORD_TRACK_INDEX]["steps"][0]
            .as_object_mut()
            .unwrap()
            .remove("chord_shape");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load(&path).unwrap();
        assert!(matches!(
            loaded.tracks[crate::model::CHORD_TRACK_INDEX].steps[0],
            Some(crate::model::StepEvent::Note {
                chord_shape: None,
                ..
            })
        ));
    }

    #[test]
    fn single_and_dyad_chord_shapes_use_stable_schema_names() {
        use crate::model::ChordShape;

        for (shape, name) in [
            (ChordShape::Single, "single"),
            (ChordShape::DyadThird, "dyad_third"),
            (ChordShape::DyadFifth, "dyad_fifth"),
        ] {
            let encoded = serde_json::to_value(shape).unwrap();
            assert_eq!(encoded, name);
            assert_eq!(
                serde_json::from_value::<ChordShape>(encoded).unwrap(),
                shape
            );
        }
    }

    #[test]
    fn arpeggio_schema_round_trips_note_values_and_omits_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arpeggio.groove.json");
        let mut project = Project::new();
        project.patterns[0].tracks[crate::model::CHORD_TRACK_INDEX].steps[0] =
            Some(crate::model::StepEvent::Note {
                degree: 1,
                octave: 3,
                accent: false,
                chord_shape: None,
                arpeggio: crate::model::ArpeggioConfig {
                    enabled: false,
                    r#type: crate::model::ArpeggioType::DownUp,
                    rate: crate::model::ArpeggioRate::ThirtySecond,
                },
                condition: Default::default(),
                retrigger_count: 1,
                locks: Default::default(),
            });
        save_atomic(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, project);
        let json = fs::read_to_string(path).unwrap();
        assert!(json.contains("\"arpeggio\""));
        assert!(json.contains("down_up"));
        assert!(!json.contains("arpeggio_enabled"));
    }

    #[test]
    fn current_schema_defaults_missing_flanger_and_effect_schema_is_strict() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("effects.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][0]["effects"]
            .as_object_mut()
            .unwrap()
            .remove("flanger");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.format_version, 17);
        assert_eq!(
            loaded.tracks[0].effects.flanger,
            crate::model::FlangerParameters::default()
        );
        save_atomic(&path, &loaded).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("\"flanger\""));

        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][0]["effects"]["distortion"]["drive"] = 101.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        value["tracks"][0]["effects"]["distortion"]["drive"] = 0.into();
        value["tracks"][0]["effects"]["phaser"]["unexpected"] = 1.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        value["tracks"][0]["effects"]["phaser"] =
            serde_json::to_value(crate::model::PhaserParameters::default()).unwrap();
        value["tracks"][0]["effects"]["flanger"]["unexpected"] = 1.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));
    }

    #[test]
    fn flanger_settings_round_trip_and_validate_feedback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("flanger.groove.json");
        let mut project = Project::new();
        project.tracks[0].effects.flanger = crate::model::FlangerParameters {
            rate: crate::model::Percent::new(73).unwrap(),
            delay: crate::model::Percent::new(18).unwrap(),
            depth: crate::model::Percent::new(92).unwrap(),
            feedback: crate::model::Percent::new(66).unwrap(),
            mix: crate::model::Percent::new(48).unwrap(),
        };
        save_atomic(&path, &project).unwrap();
        assert_eq!(load(&path).unwrap(), project);

        let mut value = serde_json::to_value(project).unwrap();
        value["tracks"][0]["effects"]["flanger"]["feedback"] = 91.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            load(&path),
            Err(ProjectIoError::Validation { .. })
        ));
    }
}
