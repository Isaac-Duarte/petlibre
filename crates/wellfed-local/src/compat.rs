//! MQTT 3.1 CONNECT compatibility for the captured stock firmware.
//!
//! Only the first CONNECT is normalized. The mature broker handles authentication
//! and every subsequent MQTT packet; this boundary never interprets feed commands.
use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Result, ensure};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
};

const MAX_CONNECT_BYTES: usize = 65_530;

pub async fn run(listener: TcpListener, broker: SocketAddr) -> Result<()> {
    let permits = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            incoming=listener.accept()=> {
                let (stream,_)=incoming?;
                let Ok(permit)=permits.clone().try_acquire_owned() else {continue};
                tasks.spawn(async move {
                    let _permit=permit;
                    // Do not log raw CONNECT errors or packets: credentials live here.
                    let _=relay(stream,broker).await;
                });
            }
            Some(_)=tasks.join_next(), if !tasks.is_empty()=>{}
        }
    }
}

async fn relay(mut device: TcpStream, broker: SocketAddr) -> Result<()> {
    let packet = tokio::time::timeout(Duration::from_secs(5), read_connect(&mut device)).await??;
    let mut upstream = TcpStream::connect(broker).await?;
    upstream.write_all(&packet).await?;
    tokio::io::copy_bidirectional(&mut device, &mut upstream).await?;
    Ok(())
}

async fn read_connect(stream: &mut TcpStream) -> Result<Vec<u8>> {
    ensure!(stream.read_u8().await? == 0x10, "expected CONNECT");
    let mut length = 0_usize;
    let mut complete = false;
    for index in 0..4 {
        let byte = stream.read_u8().await?;
        length |= usize::from(byte & 0x7f) << (7 * index);
        ensure!(length <= MAX_CONNECT_BYTES, "CONNECT exceeds limit");
        if byte & 0x80 == 0 {
            complete = true;
            break;
        }
    }
    ensure!(complete, "malformed remaining length");

    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    let body = normalize_connect(body)?;

    let mut packet = vec![0x10];
    let mut length = body.len();
    loop {
        #[expect(clippy::cast_possible_truncation, reason = "a remainder of 128")]
        let mut byte = (length % 128) as u8;
        length /= 128;
        if length > 0 {
            byte |= 0x80;
        }
        packet.push(byte);
        if length == 0 {
            break;
        }
    }

    packet.extend(body);
    Ok(packet)
}

fn normalize_connect(body: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(body.len() >= 10, "truncated CONNECT");
    if body.starts_with(b"\x00\x06MQIsdp\x03") {
        ensure!(body.len() >= 12, "truncated MQTT 3.1 header");
        // Stock firmware asks for a persistent 3.1 session. Local feed commands
        // must never wait in that session for a future reconnect. Force a clean
        // 3.1.1 session and preserve keepalive, identity and credentials verbatim.
        let mut normalized = b"\x00\x04MQTT\x04".to_vec();
        normalized.push(body[9] | 0x02);
        normalized.extend_from_slice(&body[10..]);
        return Ok(normalized);
    }
    ensure!(
        body.starts_with(b"\x00\x04MQTT") && matches!(body.get(6), Some(4 | 5)),
        "unsupported CONNECT protocol"
    );
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_header_is_normalized_without_changing_credentials_or_keepalive() {
        let mut legacy = b"\x00\x06MQIsdp\x03\xc0\x00\x5a".to_vec();
        legacy.extend_from_slice(b"\x00\x04TEST\x00\x04user\x00\x04pass");
        let normalized = normalize_connect(legacy.clone()).unwrap();
        assert_eq!(&normalized[..8], b"\x00\x04MQTT\x04\xc2");
        assert_eq!(&normalized[8..], &legacy[10..]);
    }

    #[test]
    fn modern_connect_is_unchanged_and_truncated_legacy_is_rejected() {
        let modern = b"\x00\x04MQTT\x04\x02\x00\x0f\x00\x01x".to_vec();
        assert_eq!(normalize_connect(modern.clone()).unwrap(), modern);
        assert!(normalize_connect(b"\x00\x06MQIsdp\x03\xc0".to_vec()).is_err());
    }
}
