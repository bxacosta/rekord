//! Recording: start, pause, resume and stop takes on a dedicated writer thread.
//!
//! Each take produces two aligned 48 kHz files:
//! - `<time>.wav`: what you hear, with all processing applied;
//! - `<time>_original.wav`: the untouched microphone signal, so a take can be
//!   re-processed later.
//!
//! The audio thread never touches the disk: it hands frames to a [`Tap`], and the
//! writer thread checkpoints the files every second (see [`wav`]), so a crash or a
//! closed terminal loses at most about one second.

mod recovery;
mod wav;

pub use recovery::recover_dir;

use crate::dsp::{FRAME_LEN, Frame, SAMPLE_RATE};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wav::WavWriter;

/// How often files are made consistent on disk (the most a crash can lose).
const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);
/// How often the OS is forced to physically write the data (protects against power loss).
const SYNC_EVERY: Duration = Duration::from_secs(5);
/// WAV files cannot exceed 4 GiB: a long take continues in a new part well before that.
const MAX_PART_BYTES: u32 = 3_800_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecState {
    Idle,
    Recording,
    Paused,
}

impl RecState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Recording,
            2 => Self::Paused,
            _ => Self::Idle,
        }
    }
}

/// Something the UI should tell the user about.
#[derive(Debug)]
pub enum RecEvent {
    Saved(Take),
    Failed(String),
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct Take {
    pub files: Vec<PathBuf>,
    pub duration: Duration,
}

enum Message {
    Start,
    Pause,
    Resume,
    Stop,
    Audio(Box<Block>),
    Shutdown,
}

struct Block {
    processed: Frame,
    original: Frame,
}

/// State visible to the UI and the audio thread. Only the writer thread changes it.
#[derive(Default)]
struct Status {
    state: AtomicU8,
    /// Samples recorded in the current take (pauses excluded).
    samples: AtomicU64,
    /// Bytes written in the current take, both files and all parts.
    bytes: AtomicU64,
    file: Mutex<Option<PathBuf>>,
}

impl Status {
    fn set_state(&self, state: RecState) {
        self.state.store(state as u8, Relaxed);
    }
}

pub struct Recorder {
    tx: Sender<Message>,
    status: Arc<Status>,
    events: Receiver<RecEvent>,
    worker: Option<JoinHandle<()>>,
    dir: PathBuf,
}

impl Recorder {
    pub fn new(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let (tx, rx) = channel();
        let (events_tx, events) = channel();
        let status = Arc::new(Status::default());
        let worker = {
            let (status, dir) = (status.clone(), dir.clone());
            thread::Builder::new().name("recorder".into()).spawn(move || Writer::new(dir, status, events_tx).run(rx))?
        };
        Ok(Self { tx, status, events, worker: Some(worker), dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn state(&self) -> RecState {
        RecState::from_u8(self.status.state.load(Relaxed))
    }

    /// Length of the current take, pauses excluded.
    pub fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.status.samples.load(Relaxed) as f64 / SAMPLE_RATE as f64)
    }

    pub fn bytes(&self) -> u64 {
        self.status.bytes.load(Relaxed)
    }

    /// The file currently being written (the processed one).
    pub fn current_file(&self) -> Option<PathBuf> {
        self.status.file.lock().unwrap().clone()
    }

    pub fn start(&self) {
        self.send(Message::Start);
    }

    pub fn pause(&self) {
        self.send(Message::Pause);
    }

    pub fn resume(&self) {
        self.send(Message::Resume);
    }

    pub fn stop(&self) {
        self.send(Message::Stop);
    }

    /// Start when idle; otherwise pause or resume.
    pub fn toggle(&self) {
        match self.state() {
            RecState::Idle => self.start(),
            RecState::Recording => self.pause(),
            RecState::Paused => self.resume(),
        }
    }

    /// Events that happened since the last call.
    pub fn events(&self) -> impl Iterator<Item = RecEvent> + '_ {
        self.events.try_iter()
    }

    /// The handle the audio thread uses to feed the recorder.
    pub fn tap(&self) -> Tap {
        Tap { tx: self.tx.clone(), status: self.status.clone() }
    }

    /// Finishes any take in progress and waits for the files to be closed.
    /// Returns the events produced while doing so.
    pub fn shutdown(mut self) -> Vec<RecEvent> {
        self.join();
        self.events.try_iter().collect()
    }

    fn send(&self, message: Message) {
        // The writer only exits on Shutdown, so a send cannot fail while `self` is alive.
        let _ = self.tx.send(message);
    }

    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.send(Message::Shutdown);
            let _ = worker.join();
        }
    }
}

impl Drop for Recorder {
    /// Even on an early return or a panic, the take in progress is closed properly.
    fn drop(&mut self) {
        self.join();
    }
}

/// Feeds audio to the recorder from the processing thread. Cheap to clone.
#[derive(Clone)]
pub struct Tap {
    tx: Sender<Message>,
    status: Arc<Status>,
}

impl Tap {
    pub fn is_recording(&self) -> bool {
        self.status.state.load(Relaxed) == RecState::Recording as u8
    }

    /// Records one frame, if a take is running. Never blocks.
    pub fn push(&self, processed: &Frame, original: &Frame) {
        if self.is_recording() {
            let _ = self.tx.send(Message::Audio(Box::new(Block { processed: *processed, original: *original })));
        }
    }
}

/// The two files of one part of a take.
struct Part {
    processed: WavWriter,
    original: WavWriter,
}

impl Part {
    fn writers(&mut self) -> [&mut WavWriter; 2] {
        [&mut self.processed, &mut self.original]
    }

    fn finish(self) -> io::Result<Vec<PathBuf>> {
        Ok(vec![self.processed.finish()?, self.original.finish()?])
    }
}

struct TakeInProgress {
    stem: String,
    part_number: u32,
    part: Part,
    finished: Vec<PathBuf>,
}

struct Writer {
    dir: PathBuf,
    status: Arc<Status>,
    events: Sender<RecEvent>,
    take: Option<TakeInProgress>,
    last_sync: Instant,
}

impl Writer {
    fn new(dir: PathBuf, status: Arc<Status>, events: Sender<RecEvent>) -> Self {
        Self { dir, status, events, take: None, last_sync: Instant::now() }
    }

    fn run(mut self, rx: Receiver<Message>) {
        let mut next_checkpoint = Instant::now() + CHECKPOINT_EVERY;
        loop {
            let timeout = next_checkpoint.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(Message::Start) => self.start(),
                Ok(Message::Pause) => self.pause(),
                Ok(Message::Resume) => self.resume(),
                Ok(Message::Stop) => self.stop(),
                Ok(Message::Audio(block)) => self.write(&block),
                Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= next_checkpoint {
                next_checkpoint = Instant::now() + CHECKPOINT_EVERY;
                let sync = self.last_sync.elapsed() >= SYNC_EVERY;
                self.checkpoint(sync);
            }
        }
        self.stop();
    }

    fn start(&mut self) {
        if self.take.is_some() {
            return;
        }
        let stem = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        match self.open_part(&stem, 1) {
            Ok((part, stem)) => {
                *self.status.file.lock().unwrap() = Some(part.processed.path().to_owned());
                self.status.samples.store(0, Relaxed);
                self.status.bytes.store(0, Relaxed);
                self.take = Some(TakeInProgress { stem, part_number: 1, part, finished: Vec::new() });
                self.status.set_state(RecState::Recording);
            }
            Err(e) => self.report(format!("Could not start recording: {e}")),
        }
    }

    fn pause(&mut self) {
        if self.take.is_some() {
            self.status.set_state(RecState::Paused);
            // A paused take is fully on disk: nothing is lost if the app is killed now.
            self.checkpoint(true);
        }
    }

    fn resume(&mut self) {
        if self.take.is_some() {
            self.status.set_state(RecState::Recording);
        }
    }

    fn stop(&mut self) {
        self.status.set_state(RecState::Idle);
        let Some(take) = self.take.take() else { return };
        *self.status.file.lock().unwrap() = None;
        let duration = Duration::from_secs_f64(self.status.samples.load(Relaxed) as f64 / SAMPLE_RATE as f64);
        let mut files = take.finished;
        match take.part.finish() {
            Ok(paths) => files.extend(paths),
            Err(e) => self.report(format!("Could not finalize the recording: {e}")),
        }
        let _ = self.events.send(RecEvent::Saved(Take { files, duration }));
    }

    fn write(&mut self, block: &Block) {
        if self.status.state.load(Relaxed) != RecState::Recording as u8 {
            return; // frames already in flight when pausing or stopping
        }
        if let Err(e) = self.try_write(block) {
            self.report(format!("Recording stopped, could not write to disk: {e}"));
            self.stop();
        }
    }

    fn try_write(&mut self, block: &Block) -> io::Result<()> {
        let Some(take) = self.take.as_mut() else { return Ok(()) };
        if take.part.processed.data_bytes() >= MAX_PART_BYTES {
            let number = take.part_number + 1;
            let next = Self::create_part(&self.dir, &take.stem, number)?;
            let full = std::mem::replace(&mut take.part, next);
            take.finished.extend(full.finish()?);
            take.part_number = number;
            *self.status.file.lock().unwrap() = Some(take.part.processed.path().to_owned());
        }
        take.part.processed.write(&block.processed)?;
        take.part.original.write(&block.original)?;
        self.status.samples.fetch_add(FRAME_LEN as u64, Relaxed);
        self.status.bytes.fetch_add(4 * FRAME_LEN as u64, Relaxed);
        Ok(())
    }

    fn checkpoint(&mut self, sync: bool) {
        let Some(take) = self.take.as_mut() else { return };
        let result = take.part.writers().into_iter().try_for_each(|w| w.checkpoint(sync));
        if sync {
            self.last_sync = Instant::now();
        }
        if let Err(e) = result {
            self.report(format!("Recording stopped, could not write to disk: {e}"));
            self.stop();
        }
    }

    /// Opens the first part, picking a free name if one with the same time already exists.
    fn open_part(&self, stem: &str, number: u32) -> io::Result<(Part, String)> {
        let mut attempt = 1;
        loop {
            let candidate = if attempt == 1 { stem.to_owned() } else { format!("{stem}_{attempt}") };
            match Self::create_part(&self.dir, &candidate, number) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
                result => return result.map(|part| (part, candidate)),
            }
        }
    }

    fn create_part(dir: &Path, stem: &str, number: u32) -> io::Result<Part> {
        let suffix = if number == 1 { String::new() } else { format!("_part{number}") };
        let processed = WavWriter::create(dir.join(format!("{stem}{suffix}.wav")), SAMPLE_RATE)?;
        let original = match WavWriter::create(dir.join(format!("{stem}{suffix}_original.wav")), SAMPLE_RATE) {
            Ok(w) => w,
            Err(e) => {
                let path = processed.path().to_owned();
                drop(processed);
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        };
        Ok(Part { processed, original })
    }

    fn report(&self, message: String) {
        let _ = self.events.send(RecEvent::Failed(message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(recorder: &Recorder, state: RecState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while recorder.state() != state {
            assert!(Instant::now() < deadline, "recorder never reached {state:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn push_frames(tap: &Tap, count: usize) {
        for _ in 0..count {
            tap.push(&[0.25; FRAME_LEN], &[0.5; FRAME_LEN]);
        }
    }

    fn data_len(path: &Path) -> u64 {
        std::fs::metadata(path).unwrap().len() - wav::HEADER_LEN
    }

    #[test]
    fn pause_excludes_audio_and_stop_saves_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(dir.path().to_owned()).unwrap();
        let tap = recorder.tap();

        push_frames(&tap, 10); // not recording yet: ignored
        recorder.start();
        wait_for(&recorder, RecState::Recording);
        push_frames(&tap, 100);
        recorder.pause();
        wait_for(&recorder, RecState::Paused);
        push_frames(&tap, 50); // paused: ignored
        recorder.resume();
        wait_for(&recorder, RecState::Recording);
        push_frames(&tap, 100);
        recorder.stop();
        wait_for(&recorder, RecState::Idle);

        let events = recorder.shutdown();
        let [RecEvent::Saved(take)] = events.as_slice() else { panic!("unexpected events: {events:?}") };
        assert_eq!(take.files.len(), 2);
        assert_eq!(take.duration, Duration::from_secs(2));
        assert!(take.files[1].to_string_lossy().ends_with("_original.wav"));
        for file in &take.files {
            assert_eq!(data_len(file), 200 * FRAME_LEN as u64 * 2);
        }
    }

    #[test]
    fn shutdown_finalizes_a_running_take() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(dir.path().to_owned()).unwrap();
        recorder.start();
        wait_for(&recorder, RecState::Recording);
        push_frames(&recorder.tap(), 30);

        let events = recorder.shutdown();
        let [RecEvent::Saved(take)] = events.as_slice() else { panic!("unexpected events: {events:?}") };
        assert_eq!(data_len(&take.files[0]), 30 * FRAME_LEN as u64 * 2);
    }

    #[test]
    fn takes_started_in_the_same_second_get_distinct_names() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(dir.path().to_owned()).unwrap();
        for _ in 0..2 {
            recorder.start();
            wait_for(&recorder, RecState::Recording);
            recorder.stop();
            wait_for(&recorder, RecState::Idle);
        }
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(files.len(), 4);
    }

    #[test]
    fn unwritable_folder_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(dir.path().join("gone")).unwrap();
        std::fs::remove_dir(dir.path().join("gone")).unwrap();
        recorder.start();
        let events = recorder.shutdown();
        assert!(matches!(events.as_slice(), [RecEvent::Failed(_)]), "{events:?}");
    }
}
