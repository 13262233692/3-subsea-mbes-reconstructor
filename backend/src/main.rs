use mbes_reconstructor::*;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,mbes_reconstructor=debug,tokio=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(false)
        .with_line_number(false)
        .init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing();
    info!("=========================================================");
    info!(" MBES Real-Time Imaging Workstation");
    info!(" Deep-sea Multibeam Echosounder Reconstructor");
    info!(" EM302 Compatible - Snell's Law Refraction Engine");
    info!("=========================================================");
    let state = SharedPipelineState::new();
    let hub = BroadcastHub::new(512);
    let udp_config = UdpReceiverConfig {
        bind_addr: "0.0.0.0:4001".parse()?,
        recv_buffer_size: 4 * 1024 * 1024,
        channel_capacity: 16384,
        simulate_if_no_data: true,
    };
    let ws_config = WebSocketServerConfig {
        bind_addr: "0.0.0.0:9001".parse()?,
        broadcast_capacity: 1024,
        batch_size: 32000,
        flush_interval_ms: 40,
    };
    let (frame_tx, mut frame_rx) = mpsc::channel::<Result<ParsedFrame, ProtocolError>>(16384);
    info!("Starting UDP receiver task on {}", udp_config.bind_addr);
    let _udp_task = tokio::spawn(async move {
        let receiver = UdpReceiver::new(udp_config, frame_tx);
        if let Err(e) = receiver.run().await {
            error!("UDP receiver fatal: {}", e);
        }
    });
    let state_for_processing = state.clone();
    info!("Starting frame processing loop (Snell refraction engine ready");
    let mut processor = FrameProcessor::new(state_for_processing.clone());
    let processing_state = state_for_processing.clone();
    let _processing_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            let mut batch = Vec::with_capacity(64);
            loop {
                let mut did_work = false;
                loop {
                    match frame_rx.try_recv() {
                        Ok(frame) => {
                            batch.push(frame);
                            did_work = true;
                            if batch.len() >= 32 {
                                break;
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            info!("Frame channel closed");
                            return;
                        }
                    }
                }
                if !batch.is_empty() {
                    let count = processor.process_frames(batch.drain(..).collect());
                    let _ = count;
                }
                if !did_work {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                let _ = &processing_state;
            }
        });
    });
    info!("Starting WebSocket broadcast server on ws://{}", ws_config.bind_addr);
    let ws_server = WebSocketServer::new(ws_config, hub.clone(), state.clone());
    let ws_task = tokio::spawn(async move {
        if let Err(e) = ws_server.run().await {
            error!("WebSocket server fatal: {}", e);
        }
    });
    let stats_task = tokio::spawn(async move {
        let mut last_pings = 0u64;
        let mut last_points = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let cur_pings = state.processed_pings.load();
            let cur_points = state.generated_points.load();
            let pps = cur_pings.saturating_sub(last_pings) / 5;
            let ptps = cur_points.saturating_sub(last_points) / 5;
            info!(
                "Pipeline: {} pings ({} pings/s), {} points ({} points/s), buf_flip={}, queued={}",
                cur_pings, pps, cur_points, ptps,
                state.point_buffer.flip_count(),
                state.point_buffer.write_len(),
            );
            last_pings = cur_pings;
            last_points = cur_points;
        }
    });
    info!("System ready. Open frontend at http://localhost:5173");
    tokio::select! {
        _ = ws_task => { info!("WebSocket server finished"); }
        _ = stats_task => { info!("Stats reporter finished"); }
    };
    Ok(())
}
