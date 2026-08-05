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
    if version != Some(10) {
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
    let _ = project.activate_pattern(0);
    project
        .validate()
        .map_err(|source| ProjectIoError::Validation {
            path: path.into(),
            source,
        })?;
    Ok(project)
}

pub fn save_atomic(path: &Path, project: &Project) -> Result<(), ProjectIoError> {
    // A caller editing outside `Editor` may still have a transient selected
    // cache. Serialize a canonical copy, never top-level track steps.
    let mut canonical = project.clone();
    let _ = canonical.store_active_pattern(0);
    canonical
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
        let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut out = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut out, &canonical).map_err(io::Error::other)?;
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
    fn reject_unknown() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(
            &f,
            r#"{"format_version":1,"globals":{},"tracks":[],"wat":1}"#,
        )
        .unwrap();
        assert!(load(&f).is_err());
    }
    #[test]
    fn v10_rejects_legacy_top_level_track_steps() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("legacy-steps.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["tracks"][0]["steps"] = serde_json::json!(vec![serde_json::Value::Null; 16]);
        fs::write(&f, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&f), Err(ProjectIoError::Json { .. })));
    }
    #[test]
    fn default_schema_uses_required_names() {
        let value = serde_json::to_value(Project::new()).unwrap();
        assert_eq!(value["format_version"], 10);
        assert_eq!(value["globals"]["key"], "C");
        assert_eq!(value["globals"]["delay_division"], "eighth");
        assert_eq!(value["globals"]["reverb_tone"], 40);
        assert_eq!(value["globals"]["reverb_pre_delay_ms"], 20);
        assert_eq!(value["tracks"].as_array().unwrap().len(), 6);
        assert_eq!(value["tracks"][0]["name"], "Kick");
        assert_eq!(value["tracks"][4]["kind"], "chord");
        assert_eq!(value["tracks"][4]["name"], "Chord");
        assert_eq!(value["tracks"][4]["reverb_send"], 20);
        assert_eq!(value["tracks"][4]["instrument"]["chorus"], "i");
        assert_eq!(value["tracks"][4]["instrument"]["sub_oscillator"], 0);
        assert_eq!(value["tracks"][5]["kind"], "lead");
        assert_eq!(value["tracks"][5]["name"], "Lead");
        assert_eq!(value["tracks"][5]["reverb_send"], 20);
        assert_eq!(value["tracks"][0]["lfos"], serde_json::json!({}));
        assert!(value["tracks"][0].get("input_degree").is_none());
        assert_eq!(
            value["patterns"].as_array().unwrap().len(),
            crate::model::MIN_PATTERN_COUNT
        );
    }

    #[test]
    fn latest_projects_without_optional_reverb_controls_use_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old-v6.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("reverb_tone");
        value["globals"]
            .as_object_mut()
            .unwrap()
            .remove("reverb_pre_delay_ms");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.globals.reverb_tone.get(), 40);
        assert_eq!(loaded.globals.reverb_pre_delay_ms, 20);
    }

    #[test]
    fn mixed_track_lengths_round_trip_and_format_v2_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("mixed.groove.json");
        let mut project = Project::new();
        for (track, length) in project.tracks.iter_mut().zip([1, 7, 16, 31, 32, 64]) {
            track.steps.resize(length, None);
        }
        save_atomic(&f, &project).unwrap();
        assert_eq!(load(&f).unwrap(), project);

        let mut value = serde_json::to_value(project).unwrap();
        value["format_version"] = 2.into();
        fs::write(&f, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            load(&f),
            Err(ProjectIoError::Validation {
                source: crate::model::ValidationError::Version(2),
                ..
            })
        ));
    }

    #[test]
    fn older_version_seven_projects_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old-pattern-bank.groove.json");
        let mut value = serde_json::to_value(Project::new()).unwrap();
        value["format_version"] = 7.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            load(&path),
            Err(ProjectIoError::Validation {
                source: crate::model::ValidationError::Version(7),
                ..
            })
        ));
    }

    #[test]
    fn older_project_versions_are_rejected_without_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.groove.json");
        for version in 1..=8 {
            let mut value = serde_json::to_value(Project::new()).unwrap();
            value["format_version"] = version.into();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(matches!(
                load(&path),
                Err(ProjectIoError::Validation {
                    source: crate::model::ValidationError::Version(v), ..
                }) if v == version
            ));
        }
    }

    #[test]
    fn bundled_projects_are_valid_latest_format_files() {
        for json in [
            include_str!("../beat"),
            include_str!("../beat2"),
            include_str!("../test1"),
            include_str!("../test2"),
        ] {
            let path = tempfile::NamedTempFile::new().unwrap();
            fs::write(path.path(), json).unwrap();
            let project = load(path.path()).unwrap();
            assert_eq!(project.format_version, 10);
            assert!(
                (crate::model::MIN_PATTERN_COUNT..=crate::model::MAX_PATTERN_COUNT)
                    .contains(&project.patterns.len())
            );
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
        project.tracks[3].lfos.cutoff = Some(crate::model::LfoConfig::default());
        project.tracks[4].lfos.pitch = Some(crate::model::LfoConfig {
            depth: crate::model::Percent::new(100).unwrap(),
            ..Default::default()
        });
        project.tracks[5].lfos.pitch = Some(crate::model::LfoConfig::default());
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
        project.tracks[4].input_chord_shape = Some(crate::model::ChordShape::SeventhRoot);
        project.tracks[4].steps[0] = Some(crate::model::StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: Some(crate::model::ChordShape::SeventhFirstInversion),
            locks: Default::default(),
        });
        save_atomic(&path, &project).unwrap();
        assert_eq!(load(&path).unwrap(), project);

        let mut value = serde_json::to_value(load(&path).unwrap()).unwrap();
        value["patterns"][0]["tracks"][4]["steps"][0]
            .as_object_mut()
            .unwrap()
            .remove("chord_shape");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load(&path).unwrap();
        assert!(matches!(
            loaded.tracks[4].steps[0],
            Some(crate::model::StepEvent::Note {
                chord_shape: None,
                ..
            })
        ));
    }

    #[test]
    fn arpeggio_schema_round_trips_base_values_and_explicit_default_locks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arpeggio.groove.json");
        let mut project = Project::new();
        if let crate::model::Instrument::Chord(parameters) = &mut project.tracks[4].instrument {
            parameters.arpeggio_enabled = true;
            parameters.arpeggio_type = crate::model::ArpeggioType::Random;
            parameters.arpeggio_rate = crate::model::ArpeggioRate::QuarterTriplet;
        }
        project.tracks[4].steps[0] = Some(crate::model::StepEvent::Note {
            degree: 1,
            octave: 3,
            accent: false,
            chord_shape: None,
            locks: crate::model::ParameterLocks {
                chord_shape: Some(crate::model::ChordShape::default()),
                arpeggio_enabled: Some(false),
                arpeggio_type: Some(crate::model::ArpeggioType::DownUp),
                arpeggio_rate: Some(crate::model::ArpeggioRate::ThirtySecond),
                ..Default::default()
            },
        });
        save_atomic(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, project);
        let json = fs::read_to_string(path).unwrap();
        assert!(json.contains("arpeggio_enabled"));
        assert!(json.contains("chord_shape"));
    }
}
