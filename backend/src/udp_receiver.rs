use crate::protocol::{ParsedFrame, StreamingParser, ProtocolError};
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

#[derive(Error, Debug)]
pub enum UdpReceiverError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Channel send error")]
    ChannelSend,
}

pub struct UdpReceiverConfig {
    pub bind_addr: SocketAddr,
    pub recv_buffer_size: usize,
    pub channel_capacity: usize,
    pub simulate_if_no_data: bool,
}

impl Default for UdpReceiverConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:4001".parse().unwrap(),
            recv_buffer_size: 2 * 1024 * 1024,
            channel_capacity: 8192,
            simulate_if_no_data: true,
        }
    }
}

pub struct UdpReceiver {
    config: UdpReceiverConfig,
    frame_tx: mpsc::Sender<Result<ParsedFrame, ProtocolError>>,
}

impl UdpReceiver {
    pub fn new(config: UdpReceiverConfig, frame_tx: mpsc::Sender<Result<ParsedFrame, ProtocolError>>) -> Self {
        Self { config, frame_tx }
    }

    pub async fn run(self) -> Result<(), UdpReceiverError> {
        info!("UDP receiver binding to {}", self.config.bind_addr);
        let socket = match UdpSocket::bind(self.config.bind_addr).await {
            Ok(s) => {
                info!("UDP socket bound successfully to {}", self.config.bind_addr);
                Arc::new(s)
            }
            Err(e) => {
                error!("Failed to bind UDP socket: {}", e);
                if self.config.simulate_if_no_data {
                    warn!("Falling back to simulated data mode");
                    return self.run_simulated().await;
                }
                return Err(UdpReceiverError::Io(e));
            }
        };
        let mut buf = vec![0u8; self.config.recv_buffer_size];
        let mut parser = StreamingParser::new();
        let mut idle_count = 0u64;
        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, src)) => {
                            debug!("Received {} bytes from {}", len, src);
                            parser.feed(&buf[..len]);
                            let frames = parser.drain_frames();
                            for frame in frames {
                                if let Err(_) = self.frame_tx.send(frame).await {
                                    warn!("Frame channel closed, exiting UDP receiver");
                                    return Ok(());
                                }
                            }
                            idle_count = 0;
                        }
                        Err(e) => {
                            error!("UDP recv error: {}", e);
                        }
                    }
                }
            }
            if self.config.simulate_if_no_data {
                idle_count += 1;
                if idle_count > 1000 {
                    warn!("No real UDP data received after {} ticks, switching to simulation", idle_count);
                    return self.run_simulated().await;
                }
            }
        }
    }

    async fn run_simulated(self) -> Result<(), UdpReceiverError> {
        info!("Starting simulated Kongsberg EM302 data stream");
        use crate::protocol::{BeamData, PingData, SvpData, SvpPoint};
        use chrono::Utc;
        use rand::Rng;
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::from_entropy();
        let svp_points: Vec<SvpPoint> = (0..60).map(|i| {
            let depth = i as f64 * 25.0;
            let t = depth / 1500.0;
            let sound_velocity = 1450.0 + 20.0 * (-((t - 0.5) * (t - 0.5)) / 0.25).exp() + rng.gen_range(-1.0..1.0);
            SvpPoint { depth, sound_velocity }
        }).collect();
        let min_v = svp_points.iter().map(|p| p.sound_velocity).fold(f64::INFINITY, f64::min);
        let max_v = svp_points.iter().map(|p| p.sound_velocity).fold(f64::NEG_INFINITY, f64::max);
        let svp_frame = ParsedFrame::Svp(SvpData {
            timestamp: Utc::now(),
            ping_number: 0,
            num_points: svp_points.len() as u16,
            points: svp_points,
            min_velocity: min_v,
            max_velocity: max_v,
        });
        if let Err(_) = self.frame_tx.send(Ok(svp_frame)).await {
            return Ok(());
        }
        info!("Initial SVP profile injected ({} layers, {:.1}-{:.1} m/s)", 60, min_v, max_v);
        let mut ping_num = 1u32;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(120)).await;
            let num_beams = 432u16;
            let mut beams = Vec::with_capacity(num_beams as usize);
            let base_angle_step = 120.0_f64 / (num_beams as f64 - 1.0);
            for b in 0..num_beams {
                let beam_angle = -60.0_f64 + b as f64 * base_angle_step;
                let theta = beam_angle.to_radians();
                let nominal_depth = 1200.0 + 180.0 * (ping_num as f64 * 0.015).sin()
                    + 35.0 * (beam_angle * 0.07).cos()
                    + rng.gen_range(-3.5..3.5);
                if (ping_num % 50) < 8 && beam_angle.abs() > 30.0 && beam_angle.abs() < 42.0 {
                    let extra = 60.0 * (1.0 - (beam_angle.abs() - 30.0) / 12.0);
                    let _ = extra;
                }
                let path_len = nominal_depth / theta.cos().max(0.1);
                let avg_sv = 1510.0;
                let travel_time = 2.0 * path_len / avg_sv;
                let seep_mask = if (ping_num % 70) >= 30 && (ping_num % 70) < 42
                    && beam_angle.abs() < 18.0 && beam_angle.abs() > 4.0 {
                    -1.0
                } else {
                    0.0
                };
                beams.push(BeamData {
                    beam_number: b,
                    travel_time: travel_time + seep_mask * 0.008,
                    tx_angle: theta,
                    rx_angle: theta + rng.gen_range(-0.003..0.003),
                    reflectivity: if seep_mask < 0.0 { -25 } else { rng.gen_range(-35..-18) },
                    quality: if beam_angle.abs() > 58.0 { 2 } else { 7 },
                });
            }
            let ping = ParsedFrame::Ping(PingData {
                timestamp: Utc::now(),
                ping_number: ping_num,
                num_beams,
                heading: (ping_num as f64 * 0.12) % 360.0,
                pitch: 0.35 * (ping_num as f64 * 0.06).sin(),
                roll: 0.6 * (ping_num as f64 * 0.09).cos(),
                heave: 0.25 * (ping_num as f64 * 0.11).sin(),
                latitude: 17.0 + (ping_num as f64) * 2.5e-5 + rng.gen_range(-1e-6..1e-6),
                longitude: -110.0 + (ping_num as f64) * 3.5e-5 + rng.gen_range(-1e-6..1e-6),
                tx_frequency: 30000.0,
                tx_beamwidth_along: 1.0,
                tx_beamwidth_across: 0.5,
                sample_rate: 48000.0,
                sound_velocity_at_tx: 1515.0,
                beams,
            });
            if let Err(_) = self.frame_tx.send(Ok(ping)).await {
                info!("Frame channel closed, exiting simulated mode");
                return Ok(());
            }
            if ping_num % 500 == 0 {
                let svp_points: Vec<SvpPoint> = (0..60).map(|i| {
                    let depth = i as f64 * 25.0;
                    let t = depth / 1500.0;
                    let sound_velocity = 1452.0 + 19.0 * (-((t - 0.52) * (t - 0.52)) / 0.24).exp() + rng.gen_range(-0.8..0.8);
                    SvpPoint { depth, sound_velocity }
                }).collect();
                let min_v = svp_points.iter().map(|p| p.sound_velocity).fold(f64::INFINITY, f64::min);
                let max_v = svp_points.iter().map(|p| p.sound_velocity).fold(f64::NEG_INFINITY, f64::max);
                let svp_frame = ParsedFrame::Svp(SvpData {
                    timestamp: Utc::now(),
                    ping_number: ping_num,
                    num_points: svp_points.len() as u16,
                    points: svp_points,
                    min_velocity: min_v,
                    max_velocity: max_v,
                });
                if let Err(_) = self.frame_tx.send(Ok(svp_frame)).await {
                    return Ok(());
                }
                debug!("Updated SVP profile at ping {}", ping_num);
            }
            ping_num = ping_num.wrapping_add(1);
        }
    }
}

pub fn spawn_udp_receiver(
    config: UdpReceiverConfig,
    _parsed_tx: crossbeam::channel::Sender<Result<ParsedFrame, ProtocolError>>,
) -> JoinHandle<()> {
    let (mpsc_tx, _mpsc_rx) = mpsc::channel::<Result<ParsedFrame, ProtocolError>>(config.channel_capacity);
    tokio::spawn(async move {
        let receiver = UdpReceiver::new(config, mpsc_tx);
        if let Err(e) = receiver.run().await {
            error!("UDP receiver fatal error: {}", e);
        }
    })
}

