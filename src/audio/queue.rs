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
    snapshot_project: AudioProject,
}

pub(super) struct AudioResources {
    pub(super) log_path: PathBuf,
    pub(super) sample_rate: u32,
    pub(super) recording_worker: super::recording::RecordingWorker,
    pub(super) snapshot_project: AudioProject,
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
            snapshot_project: resources.snapshot_project,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_queue_only(
        project: &Project,
        command_capacity: usize,
    ) -> (Self, Consumer<AudioCommand>) {
        let status = Arc::new(AudioStatus::default());
        let (recording_worker, _recording) =
            super::recording::RecordingWorker::spawn(8_000, status.clone()).unwrap();
        let (producer, commands) = rtrb::RingBuffer::new(command_capacity);
        let (retire_producer, retired) = rtrb::RingBuffer::<Box<AudioProject>>::new(1);
        drop(retire_producer);
        let (error_producer, stream_errors) = rtrb::RingBuffer::<StreamError>::new(1);
        drop(error_producer);
        (
            Self {
                stream: None,
                device_name: "test".into(),
                status,
                producer,
                retired,
                stream_errors,
                log_path: PathBuf::new(),
                sample_rate: 8_000,
                recording_worker,
                recording_state: super::RecordingState::Idle,
                recording_path: None,
                snapshot_project: AudioProject::from_project(project),
            },
            commands,
        )
    }

    pub fn send(&mut self, command: AudioCommand) -> Result<(), QueueFull> {
        self.reap_retired();
        // Full replacements are also used by project open/new. Keep the UI-side
        // incremental builder aligned only after the command has been accepted.
        let replacement = match &command {
            AudioCommand::ReplaceProject { project, .. } => Some((**project).clone()),
            _ => None,
        };
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(_)| QueueFull)?;
        if let Some(project) = replacement {
            self.snapshot_project = project;
        }
        Ok(())
    }
    pub(crate) fn send_project_update(
        &mut self,
        project: &Project,
        impact: &crate::reducer::EditImpact,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
        song_map: SongIndexMap,
    ) -> Result<(), QueueFull> {
        let next = self.snapshot_project.updated(project, impact, pattern_map);
        let command = AudioCommand::ReplaceProject {
            project: Box::new(next.clone()),
            smoothing,
            pattern_map,
            song_map,
        };
        self.send(command)?;
        Ok(())
    }
    pub fn available_commands(&self) -> usize {
        self.producer.slots()
    }
    /// Destroy snapshots on the UI thread, never from the audio callback.
    pub fn reap_retired(&mut self) {
        reap_retired_queue(&mut self.retired);
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
        if self.status.failed.load(Ordering::Acquire) {
            anyhow::bail!("audio stream has failed; cannot start recording")
        }
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
        if self.status.failed.load(Ordering::Acquire) {
            self.begin_failed_stream_finalization();
            return Ok(());
        }
        self.send(AudioCommand::StopRecording)
            .map_err(|_| super::recording::queue_full_error())?;
        self.recording_state = super::RecordingState::Finalizing;
        Ok(())
    }
    pub fn poll_recording_event(&mut self) -> Option<super::RecordingEvent> {
        if self.recording_state != super::RecordingState::Idle
            && self.status.failed.load(Ordering::Acquire)
        {
            self.begin_failed_stream_finalization();
        }
        let event = self.recording_worker.poll()?;
        self.recording_state = super::RecordingState::Idle;
        self.recording_path = None;
        Some(event)
    }
    fn begin_failed_stream_finalization(&mut self) {
        if self.recording_state == super::RecordingState::Idle || self.stream.is_none() {
            return;
        }
        self.recording_state = super::RecordingState::Finalizing;
        // A failed callback cannot deliver an ordered end marker. Stop and
        // destroy its producer first, then let the worker drain every frame it
        // had already accepted and finalize that contiguous prefix.
        self.stream.take();
        self.recording_worker.request_failed_stream_shutdown();
    }
    pub fn shutdown_recording(&mut self) {
        if self.status.failed.load(Ordering::Acquire) {
            self.begin_failed_stream_finalization();
        } else if self.recording_state == super::RecordingState::Recording {
            loop {
                self.reap_retired();
                if self.status.failed.load(Ordering::Acquire) {
                    self.begin_failed_stream_finalization();
                    break;
                }
                if self.stop_recording().is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        while self.recording_state != super::RecordingState::Idle {
            self.reap_retired();
            if self.status.failed.load(Ordering::Acquire) {
                self.begin_failed_stream_finalization();
            }
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

fn reap_retired_queue(retired: &mut Consumer<Box<AudioProject>>) {
    while let Ok(snapshot) = retired.pop() {
        drop(snapshot);
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stream.take();
        self.recording_worker.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    fn queue_only_audio(
        project: &Project,
        command_capacity: usize,
    ) -> (Audio, Consumer<AudioCommand>) {
        Audio::test_queue_only(project, command_capacity)
    }

    #[test]
    fn finalization_wait_can_release_every_retirement_slot() {
        let project = Project::new();
        let (mut producer, mut retired) = RingBuffer::new(32);
        for _ in 0..32 {
            producer
                .push(Box::new(AudioProject::from_project(&project)))
                .unwrap();
        }
        assert_eq!(producer.slots(), 0);
        reap_retired_queue(&mut retired);
        assert_eq!(producer.slots(), 32);
    }

    #[test]
    fn full_snapshot_updates_the_incremental_cache_only_after_enqueue_succeeds() {
        let initial = Project::new();
        let mut loaded = initial.clone();
        loaded.globals.tempo_bpm = 137;
        loaded.patterns[0].tracks[1].steps[0] = Some(crate::model::StepEvent::Trigger {
            accent: false,
            recipe: Default::default(),
            condition: Default::default(),
            retrigger_count: 1,
            microtiming: crate::model::Microtiming::ZERO,
            locks: Default::default(),
        });
        let mut edited = loaded.clone();
        edited.tracks[0].muted = true;

        let (mut audio, mut commands) = queue_only_audio(&initial, 4);
        audio.send(Audio::snapshot(&loaded)).unwrap();
        audio
            .send_project_update(
                &edited,
                &crate::reducer::EditImpact {
                    tracks: 1,
                    ..Default::default()
                },
                ParameterSmoothing::Default,
                PatternIndexMap::identity(),
                SongIndexMap::identity(),
            )
            .unwrap();
        let _ = commands.pop().unwrap();
        let AudioCommand::ReplaceProject { project, .. } = commands.pop().unwrap() else {
            panic!("expected project replacement")
        };
        let expected = AudioProject::from_project(&edited);
        assert_eq!(project.globals.tempo_bpm, expected.globals.tempo_bpm);
        assert_eq!(project.patterns[0], expected.patterns[0]);
        assert_eq!(project.tracks[0].muted, expected.tracks[0].muted);

        let (mut full_audio, _commands) = queue_only_audio(&initial, 1);
        full_audio.send(Audio::snapshot(&loaded)).unwrap();
        assert!(full_audio.send(Audio::snapshot(&initial)).is_err());
        assert_eq!(
            full_audio.snapshot_project.globals.tempo_bpm,
            loaded.globals.tempo_bpm
        );
    }
}
