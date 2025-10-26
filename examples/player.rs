// ABOUTME: End-to-end player example
// ABOUTME: Connects to server, receives audio, and plays it back

use resonate::audio::decode::{Decoder, PcmDecoder};
use resonate::audio::{AudioBuffer, AudioFormat, AudioOutput, Codec, CpalOutput};
use resonate::protocol::client::ProtocolClient;
use resonate::protocol::messages::{
    AudioFormatSpec, ClientHello, DeviceInfo, Message, PlayerSupport,
};
use resonate::scheduler::AudioScheduler;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let hello = ClientHello {
        client_id: uuid::Uuid::new_v4().to_string(),
        name: "Resonate-RS Player".to_string(),
        version: 1,
        supported_roles: vec!["player".to_string()],
        device_info: DeviceInfo {
            product_name: "Resonate-RS Player".to_string(),
            manufacturer: "Resonate".to_string(),
            software_version: "0.1.0".to_string(),
        },
        player_support: Some(PlayerSupport {
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

    println!("Connecting to ws://localhost:8927/resonate...");
    let mut client = ProtocolClient::connect("ws://localhost:8927/resonate", hello).await?;
    println!("Connected!");

    println!("Waiting for stream to start...");

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

    loop {
        // Process messages first
        if let Some(msg) = client.recv_message().await {
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
                }
                _ => {
                    println!("Received message: {:?}", msg);
                }
            }
            continue;
        }

        // Then process audio chunks
        if let Some(chunk) = client.recv_audio_chunk().await {
            if let (Some(ref dec), Some(ref fmt)) = (&decoder, &audio_format) {
                match dec.decode(&chunk.data) {
                    Ok(samples) => {
                        let buffer = AudioBuffer {
                            timestamp: chunk.timestamp,
                            play_at: Instant::now() + Duration::from_millis(100),
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
            continue;
        }

        // Both channels closed
        break;
    }

    // Note: playback_handle will be cleaned up when program exits
    // We don't join() here since the thread runs an infinite loop
    drop(playback_handle);
    Ok(())
}
