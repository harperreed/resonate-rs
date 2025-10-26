// ABOUTME: End-to-end player example
// ABOUTME: Connects to server, receives audio, and plays it back

use clap::Parser;
use resonate::audio::decode::{Decoder, PcmDecoder};
use resonate::audio::{AudioBuffer, AudioFormat, AudioOutput, Codec, CpalOutput};
use resonate::protocol::client::ProtocolClient;
use resonate::protocol::messages::{
    AudioFormatSpec, ClientHello, ClientTime, DeviceInfo, Message, PlayerSupport, PlayerUpdate,
};
use resonate::scheduler::AudioScheduler;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

/// Resonate audio player
#[derive(Parser, Debug)]
#[command(name = "player")]
#[command(about = "Connect to Resonate server and play audio", long_about = None)]
struct Args {
    /// WebSocket URL of the Resonate server
    #[arg(short, long, default_value = "ws://localhost:8927/resonate")]
    server: String,

    /// Client name
    #[arg(short, long, default_value = "Resonate-RS Player")]
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    let hello = ClientHello {
        client_id: uuid::Uuid::new_v4().to_string(),
        name: args.name.clone(),
        version: 1,
        supported_roles: vec!["player".to_string()],
        device_info: DeviceInfo {
            product_name: args.name.clone(),
            manufacturer: "Resonate".to_string(),
            software_version: "0.1.0".to_string(),
        },
        player_support: Some(PlayerSupport {
            support_codecs: vec!["pcm".to_string()],
            support_channels: vec![2], // Stereo
            support_sample_rates: vec![48000, 96000, 192000],
            support_bit_depth: vec![16, 24],
            support_formats: vec![AudioFormatSpec {
                codec: "pcm".to_string(),
                channels: 2,
                sample_rate: 48000,
                bit_depth: 24,
            }],
            buffer_capacity: 100,
            supported_commands: vec!["play".to_string(), "pause".to_string()],
        }),
        metadata_support: None,
    };

    println!("Connecting to {}...", args.server);
    let client = ProtocolClient::connect(&args.server, hello).await?;
    println!("Connected!");

    // Split client into separate receivers for concurrent processing
    let (mut message_rx, mut audio_rx, clock_sync, ws_tx) = client.split();

    // Send initial player/update message (handshake step 3)
    let player_update = Message::PlayerUpdate(PlayerUpdate {
        state: "idle".to_string(),
        volume: 100,
        muted: false,
    });
    ws_tx.send_message(player_update).await?;
    println!("Sent initial player/update");

    // Send immediate initial clock sync
    let client_transmitted = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    let time_msg = Message::ClientTime(ClientTime { client_transmitted });
    ws_tx.send_message(time_msg).await?;
    println!("Sent initial client/time for clock sync");

    println!("Waiting for stream to start...");

    // Spawn clock sync task that sends client/time every 5 seconds
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            // Get current Unix epoch microseconds
            let client_transmitted = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;

            let time_msg = Message::ClientTime(ClientTime { client_transmitted });

            // Send time sync message
            if let Err(e) = ws_tx.send_message(time_msg).await {
                eprintln!("Failed to send time sync: {}", e);
                break;
            }
        }
    });

    // Create shared scheduler
    let scheduler = Arc::new(AudioScheduler::new());
    let scheduler_clone = Arc::clone(&scheduler);

    // Spawn playback thread (not tokio task, since CpalOutput is !Send)
    let playback_handle = std::thread::spawn(move || {
        let mut output: Option<CpalOutput> = None;

        loop {
            if let Some(buffer) = scheduler_clone.next_ready() {
                // Lazily initialize output when first buffer arrives
                if output.is_none() {
                    match CpalOutput::new(buffer.format.clone()) {
                        Ok(out) => {
                            println!("Audio output initialized");
                            output = Some(out);
                        }
                        Err(e) => {
                            eprintln!("Failed to create audio output: {}", e);
                            break;
                        }
                    }
                }

                if let Some(ref mut out) = output {
                    if let Err(e) = out.write(&buffer.samples) {
                        eprintln!("Output error: {}", e);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    // Message handling variables
    let mut decoder: Option<PcmDecoder> = None;
    let mut audio_format: Option<AudioFormat> = None;
    let mut buffering_count = 0;
    const BUFFERING_TARGET: usize = 25; // Buffer 25 chunks (~500ms) before starting playback
    let mut next_play_time: Option<Instant> = None; // Track when next chunk should play

    loop {
        // Process messages and audio chunks concurrently
        tokio::select! {
            Some(msg) = message_rx.recv() => {
                match msg {
                    Message::StreamStart(stream_start) => {
                        println!(
                            "Stream starting: {} {}Hz {}ch {}bit",
                            stream_start.player.codec,
                            stream_start.player.sample_rate,
                            stream_start.player.channels,
                            stream_start.player.bit_depth
                        );

                        audio_format = Some(AudioFormat {
                            codec: Codec::Pcm,
                            sample_rate: stream_start.player.sample_rate,
                            channels: stream_start.player.channels,
                            bit_depth: stream_start.player.bit_depth,
                            codec_header: None,
                        });

                        decoder = Some(PcmDecoder::new(stream_start.player.bit_depth));
                        buffering_count = 0; // Reset on new stream
                        next_play_time = None;
                    }
                    Message::ServerTime(server_time) => {
                        // Get t4 (client receive time) in Unix microseconds
                        let t4 = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_micros() as i64;

                        // Update clock sync with all four timestamps
                        let t1 = server_time.client_transmitted;
                        let t2 = server_time.server_received;
                        let t3 = server_time.server_transmitted;

                        clock_sync.lock().await.update(t1, t2, t3, t4);

                        // Log sync quality
                        let sync = clock_sync.lock().await;
                        if let Some(rtt) = sync.rtt_micros() {
                            let quality = sync.quality();
                            println!(
                                "Clock sync updated: RTT={:.2}ms, quality={:?}",
                                rtt as f64 / 1000.0,
                                quality
                            );
                        }
                    }
                    _ => {
                        println!("Received message: {:?}", msg);
                    }
                }
            }
            Some(chunk) = audio_rx.recv() => {
                if let (Some(ref dec), Some(ref fmt)) = (&decoder, &audio_format) {
                    match dec.decode(&chunk.data) {
                        Ok(samples) => {
                            // Calculate chunk duration in microseconds
                            // samples.len() includes all channels
                            let frames = samples.len() / fmt.channels as usize;
                            let duration_micros = (frames as u64 * 1_000_000) / fmt.sample_rate as u64;
                            let duration = Duration::from_micros(duration_micros);

                            // Try to use clock sync to determine play_at time
                            let sync = clock_sync.lock().await;
                            let play_at = if let Some(instant) = sync.server_to_local_instant(chunk.timestamp) {
                                // Clock sync is ready, use synchronized timestamp
                                if buffering_count < BUFFERING_TARGET {
                                    // Initial buffering: keep track for logging
                                    if next_play_time.is_none() {
                                        next_play_time = Some(instant);
                                    }
                                }
                                instant
                            } else {
                                // No clock sync yet, fall back to continuous scheduling
                                if buffering_count < BUFFERING_TARGET {
                                    // Initial buffering: start 500ms in future
                                    if next_play_time.is_none() {
                                        next_play_time = Some(Instant::now() + Duration::from_millis(500));
                                    }
                                    let play_time = next_play_time.unwrap();
                                    next_play_time = Some(play_time + duration);
                                    play_time
                                } else {
                                    // After buffering: schedule immediately after previous chunk
                                    let play_time = next_play_time.unwrap_or(Instant::now());
                                    next_play_time = Some(play_time + duration);
                                    play_time
                                }
                            };
                            drop(sync); // Release lock

                            let buffer = AudioBuffer {
                                timestamp: chunk.timestamp,
                                play_at,
                                samples,
                                format: fmt.clone(),
                            };

                            scheduler.schedule(buffer);
                            buffering_count += 1;

                            if buffering_count == BUFFERING_TARGET {
                                println!("Buffering complete, starting playback!");
                            }
                        }
                        Err(e) => {
                            eprintln!("Decode error: {}", e);
                        }
                    }
                }
            }
            else => {
                // Both channels closed
                break;
            }
        }
    }

    // Note: playback_handle will be cleaned up when program exits
    // We don't join() here since the thread runs an infinite loop
    drop(playback_handle);
    Ok(())
}
