use crate::model::ProjectV4;
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
    #[error("invalid version 3 project in {path}: {message}")]
    Migration { path: PathBuf, message: String },
    #[error("could not save {path}: {source}")]
    Save {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub fn load(path: &Path) -> Result<ProjectV4, ProjectIoError> {
    load_with_info(path).map(|loaded| loaded.project)
}

#[derive(Debug)]
pub struct LoadedProject {
    pub project: ProjectV4,
    pub migrated_from: Option<u32>,
}

pub fn load_with_info(path: &Path) -> Result<LoadedProject, ProjectIoError> {
    let bytes = fs::read(path).map_err(|source| ProjectIoError::Read {
        path: path.into(),
        source,
    })?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| ProjectIoError::Json {
            path: path.into(),
            source,
        })?;
    let version = value.get("format_version").and_then(|value| value.as_u64());
    let migrated_from = match version {
        Some(4) => None,
        Some(3) => {
            migrate_v3(&mut value).map_err(|message| ProjectIoError::Migration {
                path: path.into(),
                message,
            })?;
            Some(3)
        }
        Some(version) => {
            return Err(ProjectIoError::Validation {
                path: path.into(),
                source: crate::model::ValidationError::Version(version as u32),
            });
        }
        None => None,
    };
    let project: ProjectV4 =
        serde_json::from_value(value).map_err(|source| ProjectIoError::Json {
            path: path.into(),
            source,
        })?;
    project
        .validate()
        .map_err(|source| ProjectIoError::Validation {
            path: path.into(),
            source,
        })?;
    Ok(LoadedProject {
        project,
        migrated_from,
    })
}

fn migrate_v3(value: &mut serde_json::Value) -> Result<(), String> {
    value["format_version"] = 4.into();
    let Some(tracks) = value
        .get_mut("tracks")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(());
    };
    for (track_index, track) in tracks.iter_mut().enumerate() {
        let Some(steps) = track
            .get_mut("steps")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for (step_index, step) in steps.iter_mut().enumerate() {
            let Some(event) = step.as_object_mut() else {
                continue;
            };
            if event.contains_key("velocity") {
                return Err(format!(
                    "tracks[{track_index}].steps[{step_index}]: unexpected velocity field"
                ));
            }
            if matches!(
                event.get("type").and_then(serde_json::Value::as_str),
                Some("trigger" | "note")
            ) {
                event.insert("velocity".into(), 100.into());
            }
        }
    }
    Ok(())
}

pub fn save_atomic(path: &Path, project: &ProjectV4) -> Result<(), ProjectIoError> {
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
        let p = ProjectV4::new();
        save_atomic(&f, &p).unwrap();
        assert_eq!(load(&f).unwrap(), p);
        assert!(fs::read(&f).unwrap().ends_with(b"\n"));
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
    fn default_schema_uses_required_names() {
        let value = serde_json::to_value(ProjectV4::new()).unwrap();
        assert_eq!(value["format_version"], 4);
        assert_eq!(value["globals"]["key"], "C");
        assert_eq!(value["globals"]["delay_division"], "eighth");
        assert_eq!(value["tracks"].as_array().unwrap().len(), 6);
        assert_eq!(value["tracks"][0]["name"], "Kick");
        assert_eq!(value["tracks"][0]["lfos"], serde_json::json!({}));
        assert!(value["tracks"][0].get("input_degree").is_none());
    }

    #[test]
    fn mixed_track_lengths_round_trip_and_format_v2_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("mixed.groove.json");
        let mut project = ProjectV4::new();
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
    fn format_v3_events_migrate_to_full_velocity_without_rewriting_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.groove.json");
        let mut project = ProjectV4::new();
        project.tracks[0].steps[0] = Some(crate::model::StepEvent::Trigger {
            velocity: crate::model::DEFAULT_DRUM_VELOCITY,
            locks: Default::default(),
        });
        project.tracks[3].steps[0] = Some(crate::model::StepEvent::Note {
            degree: 1,
            octave: 3,
            velocity: crate::model::DEFAULT_NOTE_VELOCITY,
            locks: Default::default(),
        });
        let mut value = serde_json::to_value(project).unwrap();
        value["format_version"] = 3.into();
        value["tracks"][0]["steps"][0]
            .as_object_mut()
            .unwrap()
            .remove("velocity");
        value["tracks"][3]["steps"][0]
            .as_object_mut()
            .unwrap()
            .remove("velocity");
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let loaded = load_with_info(&path).unwrap();
        assert_eq!(loaded.migrated_from, Some(3));
        assert_eq!(
            loaded.project.tracks[0].steps[0]
                .as_ref()
                .unwrap()
                .velocity(),
            crate::model::Percent::new(100)
        );
        assert_eq!(
            loaded.project.tracks[3].steps[0]
                .as_ref()
                .unwrap()
                .velocity(),
            crate::model::Percent::new(100)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap()["format_version"],
            3
        );
    }

    #[test]
    fn format_v3_rejects_a_velocity_field_instead_of_silently_overwriting_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-legacy.groove.json");
        let mut project = ProjectV4::new();
        project.tracks[0].steps[0] = Some(crate::model::StepEvent::Trigger {
            velocity: crate::model::DEFAULT_DRUM_VELOCITY,
            locks: Default::default(),
        });
        let mut value = serde_json::to_value(project).unwrap();
        value["format_version"] = 3.into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        assert!(matches!(
            load_with_info(&path),
            Err(ProjectIoError::Migration { .. })
        ));
    }

    #[test]
    fn lfo_schema_round_trips_synced_and_free_rates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lfos.groove.json");
        let mut project = ProjectV4::new();
        project.tracks[0].lfos.tone = Some(crate::model::LfoConfig {
            waveform: crate::model::LfoWaveform::SampleAndHold,
            rate: crate::model::LfoRate::Free {
                rate_percent: crate::model::Percent::new(75).unwrap(),
            },
            depth: crate::model::Percent::new(40).unwrap(),
            enabled: true,
        });
        project.tracks[3].lfos.cutoff = Some(crate::model::LfoConfig::default());
        save_atomic(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, project);
        let json = fs::read_to_string(path).unwrap();
        assert!(json.contains("sample_and_hold"));
        assert!(json.contains("rate_percent"));
        assert!(json.contains("quarter"));
    }

    #[test]
    fn lfo_schema_rejects_unknown_and_missing_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-lfo.groove.json");
        let mut value = serde_json::to_value(ProjectV4::new()).unwrap();
        value["tracks"][0]["lfos"]["wat"] =
            serde_json::to_value(crate::model::LfoConfig::default()).unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));

        let mut value = serde_json::to_value(ProjectV4::new()).unwrap();
        value["tracks"][0].as_object_mut().unwrap().remove("lfos");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(ProjectIoError::Json { .. })));
    }
}
