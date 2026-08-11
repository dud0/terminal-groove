use super::{AudioStatus, QueueFull};
use anyhow::{Context, Result, bail};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    fs::{self, File, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) const RECORDING_DIRECTORY_NAME: &str = ".recordings";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Finalizing,
}

#[derive(Debug)]
pub struct RecordingEvent {
    pub path: PathBuf,
    pub frames: u64,
    pub result: Result<(), String>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RecordingItem {
    Frame(f32, f32),
    End { overflowed: bool },
}

pub(super) struct RecordingProducer {
    queue: Producer<RecordingItem>,
    status: Arc<AudioStatus>,
    active: bool,
}

impl RecordingProducer {
    pub(super) fn disconnected(status: Arc<AudioStatus>) -> Self {
        let (queue, consumer) = RingBuffer::new(1);
        drop(consumer);
        Self {
            queue,
            status,
            active: false,
        }
    }

    pub(super) fn new(queue: Producer<RecordingItem>, status: Arc<AudioStatus>) -> Self {
        Self {
            queue,
            status,
            active: false,
        }
    }

    pub(super) fn start(&mut self) {
        if !self.active {
            self.active = true;
            self.status
                .recording_stop_requested
                .store(false, Ordering::Release);
            self.status
                .recording_finalizing
                .store(false, Ordering::Release);
            self.status.recording_active.store(true, Ordering::Release);
        }
    }

    pub(super) fn stop(&mut self) {
        self.end(false);
    }

    #[inline]
    pub(super) fn capture(&mut self, left: f32, right: f32) {
        if !self.active {
            return;
        }
        if self.status.recording_stop_requested.load(Ordering::Acquire) {
            self.end(false);
            return;
        }
        // One slot is permanently reserved for the ordered end marker.
        if self.queue.slots() <= 1 || self.queue.push(RecordingItem::Frame(left, right)).is_err() {
            self.end(true);
        }
    }

    fn end(&mut self, overflowed: bool) {
        if !self.active {
            return;
        }
        let pushed = self.queue.push(RecordingItem::End { overflowed }).is_ok();
        self.active = false;
        self.status.recording_active.store(false, Ordering::Release);
        self.status
            .recording_finalizing
            .store(pushed, Ordering::Release);
    }
}

type Wav = hound::WavWriter<BufWriter<File>>;

struct PreparedTake {
    path: PathBuf,
    writer: Wav,
}

enum WorkerCommand {
    Prepare(PreparedTake),
    Shutdown { take_error: Option<&'static str> },
}

struct CurrentTake {
    path: PathBuf,
    writer: Option<Wav>,
    frames: u64,
    frames_since_flush: u32,
    first_error: Option<String>,
}

impl CurrentTake {
    fn write_frame(&mut self, left: f32, right: f32, sample_rate: u32) {
        if self.first_error.is_some() {
            return;
        }
        let left = pcm24(left);
        let right = pcm24(right);
        let result = self
            .writer
            .as_mut()
            .expect("live take has writer")
            .write_sample(left)
            .and_then(|_| {
                self.writer
                    .as_mut()
                    .expect("live take has writer")
                    .write_sample(right)
            });
        match result {
            Ok(()) => {
                self.frames += 1;
                self.frames_since_flush += 1;
                if self.frames_since_flush >= sample_rate {
                    if let Err(error) = self.writer.as_mut().expect("live take has writer").flush()
                    {
                        self.first_error =
                            Some(format!("could not checkpoint WAV header: {error}"));
                    }
                    self.frames_since_flush = 0;
                }
            }
            Err(error) => self.first_error = Some(format!("could not write WAV data: {error}")),
        }
    }

    fn finish(mut self, overflowed: bool) -> RecordingEvent {
        if overflowed && self.first_error.is_none() {
            self.first_error = Some(
                "recording queue overflowed; the contiguous captured prefix was retained".into(),
            );
        }
        if let Some(mut writer) = self.writer.take() {
            if let Err(error) = writer.flush() {
                self.first_error
                    .get_or_insert_with(|| format!("could not flush WAV header: {error}"));
            }
            if let Err(error) = writer.finalize() {
                self.first_error
                    .get_or_insert_with(|| format!("could not finalize WAV: {error}"));
            }
        }
        RecordingEvent {
            path: self.path,
            frames: self.frames,
            result: self.first_error.map_or(Ok(()), Err),
        }
    }
}

pub(super) struct RecordingWorker {
    commands: Sender<WorkerCommand>,
    events: Receiver<RecordingEvent>,
    join: Option<JoinHandle<()>>,
}

impl RecordingWorker {
    pub(super) fn spawn(
        sample_rate: u32,
        status: Arc<AudioStatus>,
    ) -> Result<(Self, RecordingProducer)> {
        let capacity = (sample_rate as usize)
            .saturating_mul(2)
            .saturating_add(1)
            .max(2);
        let (producer, consumer) = RingBuffer::new(capacity);
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker_status = status.clone();
        let join = thread::Builder::new()
            .name("terminal-groove-wav-writer".into())
            .spawn(move || writer_loop(sample_rate, consumer, command_rx, event_tx, worker_status))
            .context("could not start WAV writer thread")?;
        Ok((
            Self {
                commands: command_tx,
                events: event_rx,
                join: Some(join),
            },
            RecordingProducer::new(producer, status),
        ))
    }

    pub(super) fn prepare(&self, path: &Path, sample_rate: u32) -> Result<()> {
        let parent = path
            .parent()
            .context("recording destination has no parent directory")?;
        fs::create_dir_all(parent).with_context(|| {
            format!("could not create recording directory {}", parent.display())
        })?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("could not create recording {}", path.display()))?;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::new(BufWriter::new(file), spec)
            .with_context(|| format!("could not initialize recording {}", path.display()))?;
        self.commands
            .send(WorkerCommand::Prepare(PreparedTake {
                path: path.to_owned(),
                writer,
            }))
            .map_err(|_| anyhow::anyhow!("WAV writer thread stopped"))
    }

    pub(super) fn poll(&self) -> Option<RecordingEvent> {
        self.events.try_recv().ok()
    }

    pub(super) fn request_shutdown(&self) {
        let _ = self
            .commands
            .send(WorkerCommand::Shutdown { take_error: None });
    }

    pub(super) fn request_failed_stream_shutdown(&self) {
        let _ = self.commands.send(WorkerCommand::Shutdown {
            take_error: Some("audio stream failed while recording"),
        });
    }

    pub(super) fn shutdown(&mut self) {
        self.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RecordingWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn writer_loop(
    sample_rate: u32,
    mut queue: Consumer<RecordingItem>,
    commands: Receiver<WorkerCommand>,
    events: Sender<RecordingEvent>,
    status: Arc<AudioStatus>,
) {
    let mut current: Option<CurrentTake> = None;
    let mut shutdown = false;
    let mut shutdown_error = None;
    loop {
        loop {
            match commands.try_recv() {
                Ok(WorkerCommand::Prepare(take)) => {
                    if let Some(previous) = current.take() {
                        let _ = events.send(previous.finish(false));
                    }
                    current = Some(CurrentTake {
                        path: take.path,
                        writer: Some(take.writer),
                        frames: 0,
                        frames_since_flush: 0,
                        first_error: None,
                    });
                }
                Ok(WorkerCommand::Shutdown { take_error }) => {
                    shutdown = true;
                    shutdown_error = take_error;
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let mut progressed = false;
        if current.is_some() {
            while let Ok(item) = queue.pop() {
                progressed = true;
                match item {
                    RecordingItem::Frame(left, right) => {
                        let take = current.as_mut().unwrap();
                        take.write_frame(left, right, sample_rate);
                        if take.first_error.is_some() {
                            status
                                .recording_stop_requested
                                .store(true, Ordering::Release);
                        }
                    }
                    RecordingItem::End { overflowed } => {
                        let event = current.take().unwrap().finish(overflowed);
                        status.recording_finalizing.store(false, Ordering::Release);
                        let _ = events.send(event);
                        break;
                    }
                }
            }
        }
        if shutdown {
            if let (Some(take), Some(error)) = (current.as_mut(), shutdown_error) {
                take.first_error.get_or_insert_with(|| error.into());
            }
            while let Ok(item) = queue.pop() {
                if let Some(take) = current.as_mut() {
                    match item {
                        RecordingItem::Frame(left, right) => {
                            take.write_frame(left, right, sample_rate)
                        }
                        RecordingItem::End { overflowed } => {
                            let event = current.take().unwrap().finish(overflowed);
                            let _ = events.send(event);
                        }
                    }
                }
            }
            if let Some(take) = current.take() {
                let _ = events.send(take.finish(false));
            }
            status.recording_active.store(false, Ordering::Release);
            status.recording_finalizing.store(false, Ordering::Release);
            status
                .recording_stop_requested
                .store(false, Ordering::Release);
            return;
        }
        if !progressed {
            thread::park_timeout(Duration::from_millis(1));
        }
    }
}

fn pcm24(sample: f32) -> i32 {
    if !sample.is_finite() {
        return 0;
    }
    (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32
}

pub(super) fn suggested_path(project: Option<&Path>) -> Result<PathBuf> {
    let directory = std::env::current_dir()?.join(RECORDING_DIRECTORY_NAME);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    unique_path(&directory, &project_name(project), timestamp)
}

fn project_name(path: Option<&Path>) -> String {
    let Some(name) = path
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
    else {
        return "untitled".into();
    };
    let base = name.strip_suffix(".groove.json").unwrap_or(name);
    let mut sanitized = String::with_capacity(base.len());
    let mut separator = false;
    for character in base.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            sanitized.push(character);
            separator = false;
        } else if !separator {
            sanitized.push('_');
            separator = true;
        }
    }
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "untitled".into()
    } else {
        sanitized.into()
    }
}

fn unique_path(directory: &Path, name: &str, timestamp: u128) -> Result<PathBuf> {
    for suffix in 0_u32.. {
        let filename = if suffix == 0 {
            format!("{name}-{timestamp}.wav")
        } else {
            format!("{name}-{timestamp}-{suffix}.wav")
        };
        let path = directory.join(filename);
        if !path
            .try_exists()
            .with_context(|| format!("could not inspect {}", path.display()))?
        {
            return Ok(path);
        }
    }
    bail!("could not choose a unique recording filename")
}

pub(super) fn queue_full_error() -> anyhow::Error {
    anyhow::Error::new(QueueFull).context("could not start or stop recording")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn project_names_are_stripped_and_sanitized() {
        assert_eq!(project_name(None), "untitled");
        assert_eq!(project_name(Some(Path::new("beat.groove.json"))), "beat");
        assert_eq!(
            project_name(Some(Path::new("bad name/<>.groove.json"))),
            "untitled"
        );
        assert_eq!(
            project_name(Some(Path::new("my beat🔥.groove.json"))),
            "my_beat"
        );
    }

    #[test]
    fn unique_paths_do_not_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("beat-7.wav"), []).unwrap();
        assert_eq!(
            unique_path(directory.path(), "beat", 7).unwrap(),
            directory.path().join("beat-7-1.wav")
        );
    }

    #[test]
    fn pcm_conversion_clamps_and_silences_non_finite_values() {
        assert_eq!(pcm24(1.0), 8_388_607);
        assert_eq!(pcm24(-1.0), -8_388_607);
        assert_eq!(pcm24(2.0), 8_388_607);
        assert_eq!(pcm24(f32::NAN), 0);
    }

    #[test]
    fn writer_creates_interleaved_stereo_24_bit_wav() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/take.wav");
        let status = Arc::new(AudioStatus::default());
        let (mut worker, mut producer) = RecordingWorker::spawn(48_000, status).unwrap();
        worker.prepare(&path, 48_000).unwrap();
        producer.start();
        producer.capture(-1.0, 1.0);
        producer.capture(0.5, -0.5);
        producer.stop();

        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = worker.poll() {
                break event;
            }
            assert!(Instant::now() < deadline, "writer did not finish");
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(event.frames, 2);
        assert!(event.result.is_ok());
        worker.shutdown();

        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert_eq!(reader.duration(), 2);
        assert_eq!(
            reader
                .samples::<i32>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![-8_388_607, 8_388_607, 4_194_304, -4_194_304]
        );
    }

    #[test]
    fn empty_take_finalizes_as_valid_wav() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.wav");
        let status = Arc::new(AudioStatus::default());
        let (mut worker, mut producer) = RecordingWorker::spawn(44_100, status).unwrap();
        worker.prepare(&path, 44_100).unwrap();
        producer.start();
        producer.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.poll().is_none() {
            assert!(Instant::now() < deadline, "writer did not finish");
            thread::sleep(Duration::from_millis(1));
        }
        worker.shutdown();
        let reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), 0);
        assert_eq!(reader.spec().channels, 2);
    }

    #[test]
    fn failed_stream_shutdown_without_callback_end_finalizes_the_accepted_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("failed-stream.wav");
        let status = Arc::new(AudioStatus::default());
        let (mut worker, mut producer) = RecordingWorker::spawn(48_000, status).unwrap();
        worker.prepare(&path, 48_000).unwrap();
        producer.start();
        producer.capture(0.25, -0.25);
        producer.capture(0.5, -0.5);
        drop(producer);
        worker.request_failed_stream_shutdown();

        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = worker.poll() {
                break event;
            }
            assert!(Instant::now() < deadline, "writer did not finish");
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(event.frames, 2);
        assert_eq!(
            event.result,
            Err("audio stream failed while recording".into())
        );
        worker.shutdown();
        assert_eq!(hound::WavReader::open(path).unwrap().duration(), 2);
    }

    #[test]
    fn periodic_flush_checkpoints_the_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.wav");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let writer = hound::WavWriter::new(
            BufWriter::new(file),
            hound::WavSpec {
                channels: 2,
                sample_rate: 4,
                bits_per_sample: 24,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        let mut take = CurrentTake {
            path: path.clone(),
            writer: Some(writer),
            frames: 0,
            frames_since_flush: 0,
            first_error: None,
        };
        for _ in 0..4 {
            take.write_frame(0.25, -0.25, 4);
        }
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.duration(), 4);
        drop(reader);
        assert!(take.finish(false).result.is_ok());
    }

    #[test]
    fn overflow_keeps_a_contiguous_prefix_and_ordered_end_marker() {
        let status = Arc::new(AudioStatus::default());
        let (queue, mut consumer) = RingBuffer::new(3);
        let mut producer = RecordingProducer::new(queue, status.clone());
        producer.start();
        producer.capture(1.0, -1.0);
        producer.capture(0.5, -0.5);
        producer.capture(0.25, -0.25);
        assert!(!status.recording_active.load(Ordering::Acquire));
        assert!(status.recording_finalizing.load(Ordering::Acquire));
        assert!(matches!(
            consumer.pop(),
            Ok(RecordingItem::Frame(1.0, -1.0))
        ));
        assert!(matches!(
            consumer.pop(),
            Ok(RecordingItem::Frame(0.5, -0.5))
        ));
        assert!(matches!(
            consumer.pop(),
            Ok(RecordingItem::End { overflowed: true })
        ));
    }
}
