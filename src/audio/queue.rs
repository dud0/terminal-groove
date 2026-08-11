use super::{
    AudioCommand, AudioProject, AudioStatus, ParameterSmoothing, PatternIndexMap, SongIndexMap,
};
use crate::model::Project;
use cpal::{Stream, StreamError};
use rtrb::{Consumer, Producer};
use std::{path::Path, sync::Arc};

pub struct Audio {
    pub stream: Stream,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    pub(super) producer: Producer<AudioCommand>,
    pub(super) retired: Consumer<Box<AudioProject>>,
    pub(super) stream_errors: Consumer<StreamError>,
    pub log_path: std::path::PathBuf,
}
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("audio command queue full")]
pub struct QueueFull;
impl Audio {
    pub(super) fn new(
        stream: Stream,
        device_name: String,
        status: Arc<AudioStatus>,
        producer: Producer<AudioCommand>,
        retired: Consumer<Box<AudioProject>>,
        stream_errors: Consumer<StreamError>,
        log_path: std::path::PathBuf,
    ) -> Self {
        Self {
            stream,
            device_name,
            status,
            producer,
            retired,
            stream_errors,
            log_path,
        }
    }

    pub fn send(&mut self, command: AudioCommand) -> Result<(), QueueFull> {
        self.reap_retired();
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| QueueFull)
    }
    pub fn available_commands(&self) -> usize {
        self.producer.slots()
    }
    /// Destroy snapshots on the UI thread, never from the audio callback.
    pub fn reap_retired(&mut self) {
        while let Ok(snapshot) = self.retired.pop() {
            drop(snapshot);
        }
    }
    pub fn log_pending_diagnostics(&mut self) -> std::io::Result<super::AudioDiagnostics> {
        let mut diagnostics = super::AudioDiagnostics::default();
        while let Ok(error) = self.stream_errors.pop() {
            super::append_audio_log(
                &self.log_path,
                &self.device_name,
                "runtime stream failure",
                &error.to_string(),
            )?;
            diagnostics.stream_failures += 1;
        }
        if self
            .status
            .non_finite
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            super::append_audio_log(
                &self.log_path,
                &self.device_name,
                "dsp diagnostic",
                "Audio DSP produced a non-finite value; output was silenced",
            )?;
            diagnostics.non_finite = true;
        }
        Ok(diagnostics)
    }
    pub fn audio_log_path(&self) -> &Path {
        &self.log_path
    }
    pub fn snapshot(project: &Project) -> AudioCommand {
        Self::snapshot_with_smoothing_and_maps(
            project,
            ParameterSmoothing::Default,
            PatternIndexMap::identity(),
            SongIndexMap::identity(),
        )
    }
    pub fn snapshot_with_smoothing(
        project: &Project,
        smoothing: ParameterSmoothing,
    ) -> AudioCommand {
        Self::snapshot_with_smoothing_and_maps(
            project,
            smoothing,
            PatternIndexMap::identity(),
            SongIndexMap::identity(),
        )
    }
    pub fn snapshot_with_smoothing_and_map(
        project: &Project,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    ) -> AudioCommand {
        Self::snapshot_with_smoothing_and_maps(
            project,
            smoothing,
            pattern_map,
            SongIndexMap::identity(),
        )
    }
    pub fn snapshot_with_smoothing_and_maps(
        project: &Project,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
        song_map: SongIndexMap,
    ) -> AudioCommand {
        AudioCommand::ReplaceProject {
            project: Box::new(AudioProject::from_project(project)),
            smoothing,
            pattern_map,
            song_map,
        }
    }
}
