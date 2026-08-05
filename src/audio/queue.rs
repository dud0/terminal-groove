use super::{AudioCommand, AudioProject, AudioStatus, ParameterSmoothing, PatternIndexMap};
use crate::model::Project;
use cpal::Stream;
use rtrb::{Consumer, Producer};
use std::sync::Arc;

pub struct Audio {
    pub stream: Stream,
    pub device_name: String,
    pub status: Arc<AudioStatus>,
    pub(super) producer: Producer<AudioCommand>,
    pub(super) retired: Consumer<Box<AudioProject>>,
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
    ) -> Self {
        Self {
            stream,
            device_name,
            status,
            producer,
            retired,
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
    pub fn snapshot(project: &Project) -> AudioCommand {
        Self::snapshot_with_smoothing_and_map(
            project,
            ParameterSmoothing::Default,
            PatternIndexMap::identity(),
        )
    }
    pub fn snapshot_with_smoothing(
        project: &Project,
        smoothing: ParameterSmoothing,
    ) -> AudioCommand {
        Self::snapshot_with_smoothing_and_map(project, smoothing, PatternIndexMap::identity())
    }
    pub fn snapshot_with_smoothing_and_map(
        project: &Project,
        smoothing: ParameterSmoothing,
        pattern_map: PatternIndexMap,
    ) -> AudioCommand {
        AudioCommand::ReplaceProject {
            project: Box::new(AudioProject::from_project(project)),
            smoothing,
            pattern_map,
        }
    }

    #[cfg(test)]
    pub fn snapshot_for_pattern(project: &Project, _active_pattern: usize) -> AudioCommand {
        Self::snapshot(project)
    }
}
