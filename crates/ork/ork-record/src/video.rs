use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::compressor::CompressError;

const RING_BUFFER_CAPACITY: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub pts: i64,
}

struct RecorderState {
    running: bool,
    frames: VecDeque<Frame>,
    total_bytes: usize,
    capacity: usize,
}

impl RecorderState {
    fn new(capacity: usize) -> Self {
        Self {
            running: false,
            frames: VecDeque::new(),
            total_bytes: 0,
            capacity,
        }
    }

    fn push(&mut self, frame: Frame) {
        self.total_bytes += frame.data.len();
        self.frames.push_back(frame);
        while self.total_bytes > self.capacity {
            if let Some(evicted) = self.frames.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.data.len());
            }
        }
    }

    fn drain(&mut self) -> Vec<Frame> {
        self.total_bytes = 0;
        self.frames.drain(..).collect()
    }
}

pub struct VideoRecorder {
    state: Mutex<RecorderState>,
}

impl VideoRecorder {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RecorderState::new(RING_BUFFER_CAPACITY)),
        }
    }

    pub fn start(&self) {
        self.state.lock().running = true;
        tracing::info!("video recorder started");
    }

    pub fn stop(&self) {
        self.state.lock().running = false;
        tracing::info!("video recorder stopped");
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().running
    }

    pub fn push_frame(&self, data: Vec<u8>, pts: i64) {
        self.state.lock().push(Frame { data, pts });
    }

    pub fn flush(&self) -> Vec<Frame> {
        let frames = self.state.lock().drain();
        tracing::info!(count = frames.len(), "video buffer flushed");
        frames
    }

    pub fn flush_to_cas(
        &self,
        cas: &ork_storage::cas::CasBlobStore,
    ) -> Result<Vec<String>, CompressError> {
        let frames = self.flush();
        if frames.is_empty() {
            return Ok(Vec::new());
        }

        let cas = cas.clone();
        frames
            .into_iter()
            .map(|frame| {
                let compressed = crate::compressor::ZstdCompressor::compress(&frame.data)?;
                cas.store(&compressed, false)
                    .map_err(CompressError::from)
            })
            .collect()
    }
}

impl Default for VideoRecorder {
    fn default() -> Self {
        Self::new()
    }
}
