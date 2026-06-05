//! Minimal MQTT 3.1.1 subscriber using raw TCP.
//!
//! Designed to receive real-time text (e.g., speech-to-text transcriptions)
//! from an external audio service that publishes to an MQTT broker.
//!
//! Pipeline:
//!   Audio Service (PUB) → MQTT Broker → Editor (SUB) → Register "i" / CodeLlm

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// ── Public API ─────────────────────────────────────────────────────

/// Spawn an MQTT subscriber in a background thread with its own
/// tokio runtime. Returns a `std::sync::mpsc::Receiver` that yields
/// the UTF-8 payload of each received PUBLISH message.
///
/// Follows the same async→sync bridge pattern as `spawn_lsp_task`.
pub fn spawn_mqtt_subscriber(
    host: &str,
    port: u16,
    topic: &str,
) -> Result<std::sync::mpsc::Receiver<String>, String> {
    let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (sync_tx, sync_rx): (
        std::sync::mpsc::Sender<String>,
        std::sync::mpsc::Receiver<String>,
    ) = std::sync::mpsc::channel();

    // ── Bridge: async channel → sync channel ──────────────────────
    std::thread::spawn(move || {
        while let Some(msg) = async_rx.blocking_recv() {
            if sync_tx.send(msg).is_err() {
                break;
            }
        }
    });

    let host = host.to_string();
    let topic = topic.to_string();

    // ── MQTT subscriber in its own tokio runtime ──────────────────
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create MQTT tokio runtime");

        runtime.block_on(async move {
            if let Err(e) = mqtt_subscribe_loop(&host, port, &topic, async_tx).await {
                log::error!("[MQTT] Subscriber exited with error: {}", e);
            }
        });
    });

    Ok(sync_rx)
}

// ── Internal: connection loop with auto-reconnect ──────────────────

async fn mqtt_subscribe_loop(
    host: &str,
    port: u16,
    topic: &str,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    let addr = format!("{}:{}", host, port);

    loop {
        match mqtt_connect_and_subscribe(&addr, topic, &tx).await {
            Ok(()) => {
                log::warn!("[MQTT] Connection to audio service closed, reconnecting in 5 s…");
            }
            Err(e) => {
                log::error!(
                    "[MQTT] Audio service connection error: {} — reconnecting in 5 s…",
                    e
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn mqtt_connect_and_subscribe(
    addr: &str,
    topic: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("Connect to {} failed: {}", addr, e))?;

    // ── CONNECT ───────────────────────────────────────────────────
    let client_id = format!("ce-mqtt-{}", std::process::id());
    let connect_pkt = build_connect_packet(&client_id)?;
    stream
        .write_all(&connect_pkt)
        .await
        .map_err(|e| format!("Write CONNECT failed: {}", e))?;

    let connack = read_packet(&mut stream).await?;
    if connack.is_empty() || (connack[0] >> 4) != 2 {
        return Err(format!(
            "Expected CONNACK, got: {:?}",
            connack.first().map(|b| *b >> 4)
        ));
    }

    // ── SUBSCRIBE ─────────────────────────────────────────────────
    let subscribe_pkt = build_subscribe_packet(1, topic, 0)?;
    stream
        .write_all(&subscribe_pkt)
        .await
        .map_err(|e| format!("Write SUBSCRIBE failed: {}", e))?;

    let suback = read_packet(&mut stream).await?;
    if suback.is_empty() || (suback[0] >> 4) != 9 {
        return Err(format!(
            "Expected SUBACK, got: {:?}",
            suback.first().map(|b| *b >> 4)
        ));
    }

    log::info!(
        "[MQTT] Subscribed to '{}' on {} — waiting for audio service…",
        topic,
        addr
    );

    // ── Read loop with keepalive ──────────────────────────────────
    let keepalive_interval = std::time::Duration::from_secs(30);
    let mut last_comm = std::time::Instant::now();

    loop {
        // Send PINGREQ if we haven't written anything recently
        if last_comm.elapsed() >= keepalive_interval {
            let pingreq: [u8; 2] = [0xC0, 0x00];
            stream
                .write_all(&pingreq)
                .await
                .map_err(|e| format!("Write PINGREQ failed: {}", e))?;
            last_comm = std::time::Instant::now();
            log::trace!("[MQTT] Sent PINGREQ");
        }

        // Read with a timeout so we can check keepalive periodically
        match tokio::time::timeout(std::time::Duration::from_secs(10), read_packet(&mut stream))
            .await
        {
            Ok(Ok(pkt)) => {
                last_comm = std::time::Instant::now();
                if pkt.is_empty() {
                    return Err("Empty packet — connection likely closed".into());
                }
                let pkt_type = pkt[0] >> 4;
                match pkt_type {
                    3 => {
                        // PUBLISH — message from the audio service
                        if let Some(payload) = parse_publish_payload(&pkt) {
                            log::debug!(
                                "[MQTT] Received from audio service on '{}': {} bytes",
                                topic,
                                payload.len()
                            );
                            let _ = tx.send(payload);
                        }
                    }
                    13 => {
                        // PINGRESP — no action needed
                        log::trace!("[MQTT] Received PINGRESP");
                    }
                    other => {
                        log::trace!("[MQTT] Ignoring packet type {}", other);
                    }
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // Timeout — loop back to check keepalive
            }
        }
    }
}

// ── MQTT 3.1.1 Packet Builders ────────────────────────────────────

fn build_connect_packet(client_id: &str) -> Result<Vec<u8>, String> {
    let mut var_header = Vec::new();
    // Protocol Name: "MQTT"
    var_header.extend_from_slice(&[0x00, 0x04]);
    var_header.extend_from_slice(b"MQTT");
    // Protocol Level: 4 (MQTT 3.1.1)
    var_header.push(0x04);
    // Connect Flags: Clean Session = 1 (bit 1)
    var_header.push(0x02);
    // Keep Alive: 60 s
    var_header.extend_from_slice(&[0x00, 0x3C]);

    // Payload: Client ID
    let mut payload = Vec::new();
    let id_bytes = client_id.as_bytes();
    payload.extend_from_slice(&[(id_bytes.len() >> 8) as u8, (id_bytes.len() & 0xFF) as u8]);
    payload.extend_from_slice(id_bytes);

    let remaining = var_header.len() + payload.len();
    let mut pkt = Vec::with_capacity(2 + remaining);
    pkt.push(0x10); // Packet type: CONNECT
    pkt.extend(encode_remaining_length(remaining));
    pkt.extend(var_header);
    pkt.extend(payload);
    Ok(pkt)
}

fn build_subscribe_packet(packet_id: u16, topic: &str, qos: u8) -> Result<Vec<u8>, String> {
    // Variable header: Packet Identifier
    let mut var_header = Vec::new();
    var_header.push((packet_id >> 8) as u8);
    var_header.push((packet_id & 0xFF) as u8);

    // Payload: Topic filter + Requested QoS
    let mut payload = Vec::new();
    let topic_bytes = topic.as_bytes();
    payload.extend_from_slice(&[
        (topic_bytes.len() >> 8) as u8,
        (topic_bytes.len() & 0xFF) as u8,
    ]);
    payload.extend_from_slice(topic_bytes);
    payload.push(qos);

    let remaining = var_header.len() + payload.len();
    let mut pkt = Vec::with_capacity(2 + remaining);
    pkt.push(0x82); // Packet type: SUBSCRIBE (8), reserved flags: 2
    pkt.extend(encode_remaining_length(remaining));
    pkt.extend(var_header);
    pkt.extend(payload);
    Ok(pkt)
}

// ── MQTT 3.1.1 Packet Parser ──────────────────────────────────────

/// Parse the payload string out of a full PUBLISH packet
/// (fixed-header byte + remaining data).
fn parse_publish_payload(pkt: &[u8]) -> Option<String> {
    if pkt.len() < 3 {
        return None;
    }

    let qos = (pkt[0] >> 1) & 0x03;
    let data = &pkt[1..]; // skip fixed-header byte

    let topic_len = ((data[0] as usize) << 8) | (data[1] as usize);
    if data.len() < 2 + topic_len {
        return None;
    }

    let mut offset = 2 + topic_len;
    // QoS 1 or 2 carries a 2-byte Packet Identifier
    if qos > 0 {
        offset += 2;
    }

    if offset > data.len() {
        return None;
    }

    String::from_utf8(data[offset..].to_vec()).ok()
}

// ── Low-level helpers ──────────────────────────────────────────────

fn encode_remaining_length(mut len: usize) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if len == 0 {
            break;
        }
    }
    encoded
}

/// Read one complete MQTT packet from the stream.
/// Returns `[fixed_header_byte, remaining_data…]`.
async fn read_packet(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    // First byte: packet type + flags
    let mut first = [0u8; 1];
    stream
        .read_exact(&mut first)
        .await
        .map_err(|e| format!("Read header failed: {}", e))?;

    // Decode variable-length "Remaining Length" (1–4 bytes)
    let mut remaining = 0usize;
    let mut multiplier = 1usize;
    loop {
        let mut byte = [0u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .map_err(|e| format!("Read remaining-length failed: {}", e))?;
        remaining += (byte[0] as usize & 0x7F) * multiplier;
        multiplier *= 128;
        if byte[0] & 0x80 == 0 {
            break;
        }
        if multiplier > 128 * 128 * 128 * 128 {
            return Err("Malformed remaining length".into());
        }
    }

    let mut payload = vec![0u8; remaining];
    if remaining > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| format!("Read payload ({} bytes) failed: {}", remaining, e))?;
    }

    let mut result = Vec::with_capacity(1 + remaining);
    result.push(first[0]);
    result.extend_from_slice(&payload);
    Ok(result)
}
