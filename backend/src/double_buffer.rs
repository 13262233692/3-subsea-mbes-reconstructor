use crate::protocol::{ParsedFrame, PingData, SvpData, ProtocolError};
use crate::point_cloud::Point3D;
use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct PingWithMeta {
    pub ping: PingData,
    pub points: Vec<Point3D>,
}

#[derive(Debug, Clone)]
pub enum BufferEvent {
    Svp(SvpData),
    Ping(PingWithMeta),
}

pub struct DoubleBuffer<T: Clone> {
    write: Mutex<Vec<T>>,
    read: Mutex<Vec<T>>,
    flip_counter: AtomicCell<u64>,
    capacity: usize,
}

impl<T: Clone> DoubleBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            write: Mutex::new(Vec::with_capacity(capacity)),
            read: Mutex::new(Vec::with_capacity(capacity)),
            flip_counter: AtomicCell::new(0),
            capacity,
        }
    }

    pub fn push(&self, item: T) {
        let mut write = self.write.lock();
        write.push(item);
        if write.len() > self.capacity {
            let drain_count = write.len() - self.capacity;
            write.drain(0..drain_count);
        }
    }

    pub fn extend<I: IntoIterator<Item = T>>(&self, iter: I) {
        let mut write = self.write.lock();
        let before = write.len();
        write.extend(iter);
        if write.len() > self.capacity {
            let overflow = write.len() - self.capacity;
            let start = write.len().saturating_sub(self.capacity);
            write.drain(0..start);
            let _ = (before, overflow);
        }
    }

    pub fn swap(&self) -> usize {
        let mut write = self.write.lock();
        let mut read = self.read.lock();
        std::mem::swap(&mut *write, &mut *read);
        write.clear();
        let count = read.len();
        self.flip_counter.fetch_add(1);
        count
    }

    pub fn snapshot(&self) -> Vec<T> {
        let read = self.read.lock();
        read.clone()
    }

    pub fn flip_count(&self) -> u64 {
        self.flip_counter.load()
    }

    pub fn write_len(&self) -> usize {
        self.write.lock().len()
    }
}

pub struct SharedPipelineState {
    pub svp_buffer: Arc<DoubleBuffer<SvpData>>,
    pub point_buffer: Arc<DoubleBuffer<Point3D>>,
    pub latest_svp: Mutex<Option<SvpData>>,
    pub processed_pings: AtomicCell<u64>,
    pub generated_points: AtomicCell<u64>,
}

impl SharedPipelineState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            svp_buffer: Arc::new(DoubleBuffer::new(128)),
            point_buffer: Arc::new(DoubleBuffer::new(1_500_000)),
            latest_svp: Mutex::new(None),
            processed_pings: AtomicCell::new(0),
            generated_points: AtomicCell::new(0),
        })
    }
}

pub struct FrameProcessor {
    state: Arc<SharedPipelineState>,
    refraction: crate::snell_refraction::SnellRefractionEngine,
}

impl FrameProcessor {
    pub fn new(state: Arc<SharedPipelineState>) -> Self {
        Self {
            state: state.clone(),
            refraction: crate::snell_refraction::SnellRefractionEngine::new(),
        }
    }

    pub fn process_frames(&mut self, frames: Vec<Result<ParsedFrame, ProtocolError>>) -> usize {
        let mut svp_updated = false;
        let mut pings_processed = 0usize;
        let mut all_new_points = Vec::with_capacity(65536);
        for frame in frames {
            match frame {
                Ok(ParsedFrame::Svp(svp)) => {
                    debug!("Processing SVP frame: {} layers, {:.1}-{:.1} m/s",
                        svp.num_points, svp.min_velocity, svp.max_velocity);
                    self.refraction.update_profile(&svp);
                    *self.state.latest_svp.lock() = Some(svp.clone());
                    self.state.svp_buffer.push(svp);
                    svp_updated = true;
                }
                Ok(ParsedFrame::Ping(ping)) => {
                    if !self.refraction.has_profile() {
                        warn!("Ping {} received before SVP, refraction disabled", ping.ping_number);
                    }
                    let points = self.refraction.correct_and_project(&ping);
                    let before = all_new_points.len();
                    all_new_points.extend(points);
                    let generated = all_new_points.len() - before;
                    self.state.generated_points.fetch_add(generated as u64);
                    self.state.processed_pings.fetch_add(1);
                    pings_processed += 1;
                }
                Ok(ParsedFrame::Other { msg_type, size }) => {
                    let _ = (msg_type, size);
                }
                Err(e) => {
                    debug!("Skipping malformed frame: {}", e);
                }
            }
        }
        if !all_new_points.is_empty() {
            self.state.point_buffer.extend(all_new_points.drain(..));
        }
        if svp_updated {
            info!("SVP profile updated (total refraction layers: {})",
                self.refraction.layer_count());
        }
        pings_processed
    }
}
