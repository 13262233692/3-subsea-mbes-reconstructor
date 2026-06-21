use crate::double_buffer::SharedPipelineState;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    PointBatch {
        #[serde(rename = "type")]
        msg_type: String,
        batch_id: u64,
        timestamp: f64,
        ping_start: u32,
        ping_end: u32,
        depth_min: f32,
        depth_max: f32,
        point_count: usize,
        points_flat: Vec<f32>,
    },
    SvpUpdate {
        #[serde(rename = "type")]
        msg_type: String,
        layer_count: usize,
        depths: Vec<f64>,
        velocities: Vec<f64>,
    },
    PipelineStats {
        #[serde(rename = "type")]
        msg_type: String,
        processed_pings: u64,
        generated_points: u64,
        buffer_flip_count: u64,
        queued_points: usize,
        timestamp: f64,
    },
    Welcome {
        #[serde(rename = "type")]
        msg_type: String,
        server_version: String,
        supported_streams: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ClientMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub request_backfill: Option<bool>,
}

pub struct WebSocketServerConfig {
    pub bind_addr: SocketAddr,
    pub broadcast_capacity: usize,
    pub batch_size: usize,
    pub flush_interval_ms: u64,
}

impl Default for WebSocketServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9001".parse().unwrap(),
            broadcast_capacity: 512,
            batch_size: 24000,
            flush_interval_ms: 35,
        }
    }
}

pub struct BroadcastHub {
    point_tx: broadcast::Sender<ServerMessage>,
    pub(crate) _svp_tx: broadcast::Sender<ServerMessage>,
    pub(crate) stats_tx: broadcast::Sender<ServerMessage>,
}

impl BroadcastHub {
    pub fn new(capacity: usize) -> Arc<Self> {
        let (point_tx, _) = broadcast::channel(capacity);
        let (_svp_tx, _) = broadcast::channel(capacity);
        let (stats_tx, _) = broadcast::channel(capacity);
        Arc::new(Self {
            point_tx,
            _svp_tx,
            stats_tx,
        })
    }

    pub fn broadcast_points(&self, msg: ServerMessage) {
        let _ = self.point_tx.send(msg);
    }

    pub fn broadcast_stats(&self, msg: ServerMessage) {
        let _ = self.stats_tx.send(msg);
    }

    pub fn subscribe_points(&self) -> broadcast::Receiver<ServerMessage> {
        self.point_tx.subscribe()
    }

    pub fn subscribe_stats(&self) -> broadcast::Receiver<ServerMessage> {
        self.stats_tx.subscribe()
    }
}

pub struct WebSocketServer {
    config: WebSocketServerConfig,
    hub: Arc<BroadcastHub>,
    state: Arc<SharedPipelineState>,
}

impl WebSocketServer {
    pub fn new(
        config: WebSocketServerConfig,
        hub: Arc<BroadcastHub>,
        state: Arc<SharedPipelineState>,
    ) -> Self {
        Self { config, hub, state }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let state_clone = self.state.clone();
        let hub_clone = self.hub.clone();
        let batch_size = self.config.batch_size;
        let flush_interval = std::time::Duration::from_millis(self.config.flush_interval_ms);
        let _broadcast_task = tokio::spawn(async move {
            let mut batch_id: u64 = 0;
            loop {
                tokio::time::sleep(flush_interval).await;
                let count = state_clone.point_buffer.swap();
                if count == 0 {
                    let _ = count;
                }
                let points = state_clone.point_buffer.snapshot();
                if points.is_empty() {
                    let stats = ServerMessage::PipelineStats {
                        msg_type: "stats".to_string(),
                        processed_pings: state_clone.processed_pings.load(),
                        generated_points: state_clone.generated_points.load(),
                        buffer_flip_count: state_clone.point_buffer.flip_count(),
                        queued_points: state_clone.point_buffer.write_len(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0),
                    };
                    hub_clone.broadcast_stats(stats);
                    continue;
                }
                let chunks: Vec<&[crate::point_cloud::Point3D]> = points.chunks(batch_size).collect();
                for chunk in chunks {
                    batch_id = batch_id.wrapping_add(1);
                    let mut flat = Vec::with_capacity(chunk.len() * 3);
                    let mut dmin = f32::INFINITY;
                    let mut dmax = f32::NEG_INFINITY;
                    let mut ps = u32::MAX;
                    let mut pe = u32::MIN;
                    for p in chunk {
                        flat.push(p.x);
                        flat.push(p.y);
                        flat.push(p.z);
                        flat.push(p.intensity);
                        flat.push(p.reflectivity);
                        flat.push(p.seep_hint);
                        flat.push(p.quality as f32);
                        if p.depth < dmin { dmin = p.depth; }
                        if p.depth > dmax { dmax = p.depth; }
                        if p.ping_number < ps { ps = p.ping_number; }
                        if p.ping_number > pe { pe = p.ping_number; }
                    }
                    let batch = ServerMessage::PointBatch {
                        msg_type: "points".to_string(),
                        batch_id,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0),
                        ping_start: if ps == u32::MAX { 0 } else { ps },
                        ping_end: if pe == u32::MIN { 0 } else { pe },
                        depth_min: if dmin == f32::INFINITY { 0.0 } else { dmin },
                        depth_max: if dmax == f32::NEG_INFINITY { 0.0 } else { dmax },
                        point_count: chunk.len(),
                        points_flat: flat,
                    };
                    hub_clone.broadcast_points(batch);
                }
                let stats = ServerMessage::PipelineStats {
                    msg_type: "stats".to_string(),
                    processed_pings: state_clone.processed_pings.load(),
                    generated_points: state_clone.generated_points.load(),
                    buffer_flip_count: state_clone.point_buffer.flip_count(),
                    queued_points: state_clone.point_buffer.write_len(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0),
                };
                hub_clone.broadcast_stats(stats);
            }
        });
        let state_for_svp = self.state.clone();
        let hub_for_svp = self.hub.clone();
        let _svp_task: JoinHandle<()> = tokio::spawn(async move {
            let mut last_count: Option<u64> = None;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                let svp_guard = state_for_svp.latest_svp.lock();
                if let Some(svp) = svp_guard.clone() {
                    let current_id = svp.timestamp.timestamp_millis() as u64;
                    let changed = last_count.map_or(true, |l| l != current_id);
                    if changed {
                        let _ = hub_for_svp._svp_tx.send(ServerMessage::SvpUpdate {
                            msg_type: "svp".to_string(),
                            layer_count: svp.num_points as usize,
                            depths: svp.points.iter().map(|p| p.depth).collect(),
                            velocities: svp.points.iter().map(|p| p.sound_velocity).collect(),
                        });
                        last_count = Some(current_id);
                        info!("Broadcasted updated SVP profile ({} layers)", svp.num_points);
                    }
                }
            }
        });
        let listener = tokio::net::TcpListener::bind(self.config.bind_addr).await?;
        info!("WebSocket server listening on ws://{}", self.config.bind_addr);
        let client_counter = Arc::new(Mutex::new(HashSet::<SocketAddr>::new()));
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("New client connected from {}", addr);
                    client_counter.lock().insert(addr);
                    let hub_clone = self.hub.clone();
                    let _cc = client_counter.clone();
                    let _addr = addr;
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, addr, hub_clone).await {
                            warn!("Client {} disconnected with error: {}", addr, e);
                        } else {
                            info!("Client {} disconnected cleanly", addr);
                        }
                        _cc.lock().remove(&addr);
                    });
                }
                Err(e) => {
                    error!("Accept error: {}", e);
                }
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    hub: Arc<BroadcastHub>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut outgoing, mut incoming) = ws_stream.split();
    let welcome = ServerMessage::Welcome {
        msg_type: "welcome".to_string(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        supported_streams: vec!["points".into(), "svp".into(), "stats".into()],
    };
    outgoing.send(Message::Text(serde_json::to_string(&welcome)?)).await?;
    let mut rx_points = hub.subscribe_points();
    let mut rx_stats = hub.subscribe_stats();
    let mut rx_svp = hub._svp_tx.subscribe();
    loop {
        tokio::select! {
            point_msg = rx_points.recv() => {
                match point_msg {
                    Ok(msg) => {
                        let s = serde_json::to_string(&msg)?;
                        if outgoing.send(Message::Text(s)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Client {} lagged {} point messages", addr, n);
                        rx_points = hub.subscribe_points();
                    }
                    Err(_) => break,
                }
            }
            stats_msg = rx_stats.recv() => {
                match stats_msg {
                    Ok(msg) => {
                        let s = serde_json::to_string(&msg)?;
                        if outgoing.send(Message::Text(s)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("Client {} lagged {} stats messages", addr, n);
                        rx_stats = hub.subscribe_stats();
                    }
                    Err(_) => break,
                }
            }
            svp_msg = rx_svp.recv() => {
                match svp_msg {
                    Ok(msg) => {
                        let s = serde_json::to_string(&msg)?;
                        if outgoing.send(Message::Text(s)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("Client {} lagged {} svp messages", addr, n);
                        rx_svp = hub._svp_tx.subscribe();
                    }
                    Err(_) => break,
                }
            }
            incoming_msg = incoming.next() => {
                match incoming_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Text(t))) => {
                        debug!("Client {} sent: {}", addr, t);
                    }
                    Some(Ok(Message::Binary(b))) => {
                        debug!("Client {} sent binary: {} bytes", addr, b.len());
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(e)) => {
                        warn!("WS incoming error from {}: {}", addr, e);
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                }
            }
        }
    }
    Ok(())
}
