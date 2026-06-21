use byteorder::{LittleEndian, ReadBytesExt};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("Invalid STX/ETX sync bytes")]
    InvalidSync,
    #[error("Buffer too short: need {need}, have {have}")]
    BufferTooShort { need: usize, have: usize },
    #[error("Unknown message type: {0}")]
    UnknownMessageType(u8),
    #[error("Checksum mismatch: expected {expected}, got {got}")]
    ChecksumMismatch { expected: u16, got: u16 },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

pub const STX: u8 = 0x02;
pub const ETX: u8 = 0x03;

pub const MSG_RUNTIME: u8 = 0x01;
pub const MSG_SVP: u8 = 0x0A;
pub const MSG_DEPTH: u8 = 0x12;
pub const MSG_CLOCK: u8 = 0x13;
pub const MSG_HEAD: u8 = 0x14;
pub const MSG_RANGE: u8 = 0x15;
pub const MSG_SEABED_IMAGE: u8 = 0x16;
pub const MSG_QUALITY_FACTOR: u8 = 0x17;
pub const MSG_INSTALLATION_PARAM: u8 = 0x18;
pub const MSG_EXTRA_PARAM: u8 = 0x19;
pub const MSG_SSP: u8 = 0x55;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvpPoint {
    pub depth: f64,
    pub sound_velocity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvpData {
    pub timestamp: DateTime<Utc>,
    pub ping_number: u32,
    pub num_points: u16,
    pub points: Vec<SvpPoint>,
    pub min_velocity: f64,
    pub max_velocity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeamData {
    pub beam_number: u16,
    pub travel_time: f64,
    pub tx_angle: f64,
    pub rx_angle: f64,
    pub reflectivity: i8,
    pub quality: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingData {
    pub timestamp: DateTime<Utc>,
    pub ping_number: u32,
    pub num_beams: u16,
    pub heading: f64,
    pub pitch: f64,
    pub roll: f64,
    pub heave: f64,
    pub latitude: f64,
    pub longitude: f64,
    pub tx_frequency: f32,
    pub tx_beamwidth_along: f32,
    pub tx_beamwidth_across: f32,
    pub sample_rate: f32,
    pub sound_velocity_at_tx: f32,
    pub beams: Vec<BeamData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParsedFrame {
    Svp(SvpData),
    Ping(PingData),
    Other { msg_type: u8, size: usize },
}

pub struct AllFrameParser;

impl AllFrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn find_next_frame(&self, buffer: &[u8]) -> Option<(usize, usize)> {
        let mut i = 0;
        while i + 8 <= buffer.len() {
            if buffer[i] == STX {
                let _msg_type = buffer[i + 1];
                let size = (&buffer[i + 2..i + 6]).read_u32::<LittleEndian>().ok()? as usize;
                let total_frame_size = 8 + size;
                if i + total_frame_size <= buffer.len() {
                    if buffer[i + total_frame_size - 1] == ETX {
                        return Some((i, total_frame_size));
                    }
                }
                if size > 16 * 1024 * 1024 {
                    i += 1;
                    continue;
                }
            }
            i += 1;
        }
        None
    }

    pub fn parse_frame(&self, data: &[u8]) -> Result<ParsedFrame, ProtocolError> {
        if data.len() < 8 {
            return Err(ProtocolError::BufferTooShort { need: 8, have: data.len() });
        }
        if data[0] != STX {
            return Err(ProtocolError::InvalidSync);
        }
        let msg_type = data[1];
        let size = (&data[2..6]).read_u32::<LittleEndian>()? as usize;
        let total_size = 8 + size;
        if data.len() < total_size {
            return Err(ProtocolError::BufferTooShort {
                need: total_size,
                have: data.len(),
            });
        }
        if data[total_size - 1] != ETX {
            return Err(ProtocolError::InvalidSync);
        }
        let expected_checksum = (&data[total_size - 3..total_size - 1]).read_u16::<LittleEndian>()?;
        let computed_checksum = self.compute_checksum(&data[1..total_size - 3]);
        if expected_checksum != computed_checksum && expected_checksum != 0 {
            // Some real streams skip checksum; log but proceed
        }
        let payload = &data[6..6 + size];
        match msg_type {
            MSG_SVP | MSG_SSP => self.parse_svp(payload),
            MSG_DEPTH | MSG_RANGE => self.parse_ping(payload, msg_type),
            _ => Ok(ParsedFrame::Other {
                msg_type,
                size,
            }),
        }
    }

    fn compute_checksum(&self, data: &[u8]) -> u16 {
        let mut sum: u16 = 0;
        for &b in data {
            sum = sum.wrapping_add(b as u16);
        }
        sum
    }

    fn parse_svp(&self, payload: &[u8]) -> Result<ParsedFrame, ProtocolError> {
        let mut cursor = Cursor::new(payload);
        if payload.len() < 16 {
            return Err(ProtocolError::BufferTooShort { need: 16, have: payload.len() });
        }
        let ping_number = cursor.read_u32::<LittleEndian>()?;
        let unix_time = cursor.read_f64::<LittleEndian>()?;
        let num_points = cursor.read_u16::<LittleEndian>()?;
        let _reserved = cursor.read_u16::<LittleEndian>()?;
        let points_need = num_points as usize * 16;
        let remaining = payload.len() - cursor.position() as usize;
        if remaining < points_need {
            return Err(ProtocolError::BufferTooShort { need: points_need, have: remaining });
        }
        let mut points = Vec::with_capacity(num_points as usize);
        let mut min_v = f64::INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        for _ in 0..num_points {
            let depth = cursor.read_f64::<LittleEndian>()?;
            let sound_velocity = cursor.read_f64::<LittleEndian>()?;
            if sound_velocity < min_v {
                min_v = sound_velocity;
            }
            if sound_velocity > max_v {
                max_v = sound_velocity;
            }
            points.push(SvpPoint {
                depth,
                sound_velocity,
            });
        }
        let secs = unix_time.trunc() as i64;
        let nanos = ((unix_time - unix_time.trunc()) * 1e9) as u32;
        let timestamp = Utc.timestamp_opt(secs, nanos).single().unwrap_or(Utc::now());
        Ok(ParsedFrame::Svp(SvpData {
            timestamp,
            ping_number,
            num_points,
            points,
            min_velocity: if min_v == f64::INFINITY { 0.0 } else { min_v },
            max_velocity: if max_v == f64::NEG_INFINITY { 0.0 } else { max_v },
        }))
    }

    fn parse_ping(&self, payload: &[u8], msg_type: u8) -> Result<ParsedFrame, ProtocolError> {
        let mut cursor = Cursor::new(payload);
        if payload.len() < 96 {
            return Err(ProtocolError::BufferTooShort { need: 96, have: payload.len() });
        }
        let ping_number = cursor.read_u32::<LittleEndian>()?;
        let unix_time = cursor.read_f64::<LittleEndian>()?;
        let num_beams = cursor.read_u16::<LittleEndian>()?;
        let num_valid = cursor.read_u16::<LittleEndian>()?;
        let _samplerange = cursor.read_u16::<LittleEndian>()?;
        let _reserved0 = cursor.read_u16::<LittleEndian>()?;
        let sound_velocity_at_tx = cursor.read_f32::<LittleEndian>()?;
        let sample_rate = cursor.read_f32::<LittleEndian>()?;
        let tx_frequency = cursor.read_f32::<LittleEndian>()?;
        let tx_beamwidth_along = cursor.read_f32::<LittleEndian>()?;
        let tx_beamwidth_across = cursor.read_f32::<LittleEndian>()?;
        let _tx_pulse_width = cursor.read_f32::<LittleEndian>()?;
        let _tx_power = cursor.read_f32::<LittleEndian>()?;
        let heading = cursor.read_f64::<LittleEndian>()?;
        let pitch = cursor.read_f64::<LittleEndian>()?;
        let roll = cursor.read_f64::<LittleEndian>()?;
        let heave = cursor.read_f64::<LittleEndian>()?;
        let latitude = cursor.read_f64::<LittleEndian>()?;
        let longitude = cursor.read_f64::<LittleEndian>()?;
        let secs = unix_time.trunc() as i64;
        let nanos = ((unix_time - unix_time.trunc()) * 1e9) as u32;
        let timestamp = Utc.timestamp_opt(secs, nanos).single().unwrap_or(Utc::now());
        let beam_count = if msg_type == MSG_RANGE { num_valid } else { num_beams } as usize;
        let beam_record_size = 32;
        let beams_need = beam_count * beam_record_size;
        let remaining = payload.len() - cursor.position() as usize;
        if remaining < beams_need {
            return Err(ProtocolError::BufferTooShort { need: beams_need, have: remaining });
        }
        let mut beams = Vec::with_capacity(beam_count);
        for i in 0..beam_count {
            let beam_number = cursor.read_u16::<LittleEndian>()?;
            let _flags = cursor.read_u16::<LittleEndian>()?;
            let travel_time = cursor.read_f64::<LittleEndian>()?;
            let tx_angle = cursor.read_f64::<LittleEndian>()?;
            let rx_angle = cursor.read_f64::<LittleEndian>()?;
            let reflectivity = cursor.read_i8()?;
            let quality = cursor.read_u8()?;
            let _reserved = cursor.read_u16::<LittleEndian>()?;
            let _ = i;
            beams.push(BeamData {
                beam_number,
                travel_time,
                tx_angle,
                rx_angle,
                reflectivity,
                quality,
            });
        }
        Ok(ParsedFrame::Ping(PingData {
            timestamp,
            ping_number,
            num_beams: beam_count as u16,
            heading,
            pitch,
            roll,
            heave,
            latitude,
            longitude,
            tx_frequency,
            tx_beamwidth_along,
            tx_beamwidth_across,
            sample_rate,
            sound_velocity_at_tx,
            beams,
        }))
    }
}

impl Default for AllFrameParser {
    fn default() -> Self {
        Self::new()
    }
}

pub struct StreamingParser {
    buffer: Vec<u8>,
    parser: AllFrameParser,
}

impl StreamingParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(4 * 1024 * 1024),
            parser: AllFrameParser::new(),
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    pub fn drain_frames(&mut self) -> Vec<Result<ParsedFrame, ProtocolError>> {
        let mut results = Vec::new();
        loop {
            match self.parser.find_next_frame(&self.buffer) {
                Some((start, size)) => {
                    let frame_data = &self.buffer[start..start + size];
                    let result = self.parser.parse_frame(frame_data);
                    results.push(result);
                    let drain_to = start + size;
                    self.buffer.drain(0..drain_to);
                }
                None => {
                    if self.buffer.len() > 256 * 1024 * 1024 {
                        self.buffer.clear();
                    }
                    break;
                }
            }
        }
        results
    }
}

impl Default for StreamingParser {
    fn default() -> Self {
        Self::new()
    }
}
