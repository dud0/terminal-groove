use super::{
    AudioCommand, AudioProject, AudioStatus, ParameterSmoothing, PatternIndexMap, SongIndexMap,
};
use crate::model::Project;
use cpal::{Stream, StreamError};
use rtrb::{Consumer, Producer};
use std::{path::Path, sync::Arc};
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};

pub struct Audio {
    pub stream: Option<Stream>,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    pub(super) producer: Producer<AudioCommand>,
    pub(super) retired: Consumer<Box<AudioProject>>,
    pub(super) stream_errors: Consumer<StreamError>,
    pub log_path: std::path::PathBuf,
    pub(super) sample_rate: u32,
    pub(super) recording_worker: super::recording::RecordingWorker,
    recording_state: super::RecordingState,
    recording_path: Option<PathBuf>,
}

pub(super) struct AudioResources {
    pub(super) log_path: PathBuf,
    pub(super) sample_rate: u32,
    pub(super) recording_worker: super::recording::RecordingWorker,
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
        resources: AudioResources,
    ) -> Self {
        Self {
            stream: Some(stream),
            device_name,
            status,
            producer,
            retired,
            stream_errors,
            log_path: resources.log_path,
            sample_rate: resources.sample_rate,
            recording_worker: resources.recording_worker,
            recording_state: super::RecordingState::Idle,
            recording_path: None,
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
    pub fn recording_state(&self) -> super::RecordingState {
        if self.recording_state == super::RecordingState::Recording
            && self.status.recording_finalizing.load(Ordering::Acquire)
        {
            super::RecordingState::Finalizing
        } else {
            self.recording_state
        }
    }
    pub fn recording_path(&self) -> Option<&Path> {
        self.recording_path.as_deref()
    }
    pub fn start_recording(&mut self, path: PathBuf) -> anyhow::Result<()> {
        if self.recording_state != super::RecordingState::Idle {
            anyhow::bail!("a recording is already active or finalizing")
        }
        if self.available_commands() == 0 {
            return Err(super::recording::queue_full_error());
        }
        self.recording_worker.prepare(&path, self.sample_rate)?;
        self.send(AudioCommand::StartRecording)
            .map_err(|_| super::recording::queue_full_error())?;
        self.recording_path = Some(path);
        self.recording_state = super::RecordingState::Recording;
        Ok(())
    }
    pub fn start_project_recording(
        &mut self,
        project_path: Option<&Path>,
    ) -> anyhow::Result<PathBuf> {
        let path = super::recording::suggested_path(project_path)?;
        self.start_recording(path.clone())?;
        Ok(path)
    }
    pub fn stop_recording(&mut self) -> anyhow::Result<()> {
        if self.recording_state != super::RecordingState::Recording {
            anyhow::bail!("no recording is active")
        }
        self.send(AudioCommand::StopRecording)
            .map_err(|_| super::recording::queue_full_error())?;
        self.recording_state = super::RecordingState::Finalizing;
        Ok(())
    }
    pub fn poll_recording_event(&mut self) -> Option<super::RecordingEvent> {
        let event = self.recording_worker.poll()?;
        self.recording_state = super::RecordingState::Idle;
        self.recording_path = None;
        Some(event)
    }
    pub fn shutdown_recording(&mut self) {
        if self.recording_state == super::RecordingState::Recording {
            while self.stop_recording().is_err() && !self.status.failed.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        while self.recording_state != super::RecordingState::Idle
            && !self.status.failed.load(Ordering::Acquire)
        {
            if self.poll_recording_event().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // Dropping the stream first guarantees that the callback producer can no
        // longer race the writer's final drain.
        self.stream.take();
        self.recording_worker.shutdown();
        let _ = self.poll_recording_event();
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

impl Drop for Audio {
    fn drop(&mut self) {
        self.stream.take();
        self.recording_worker.shutdown();
    }
}
