//! `speak`: play an audio file on a feeder running the speaker variant.
//!
//! `ffmpeg` decodes any format it knows to 16-bit mono PCM at the chosen
//! rate; this client passes the feeder's signed `PLSPK1` handshake
//! (`wellfed_core::speaker_stream`) with the firmware request key and
//! streams the samples over TCP. The feeder's buffer and TCP backpressure
//! pace the transfer, so nothing here keeps time.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use wellfed_core::speaker_stream::{self, HEADER_LEN, HELLO_LEN, PORT, RATES, Status};
use serde_json::Value;

use crate::config::Config;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The feeder's address on the hotspot: its MAC in the interface's neighbour table.
pub fn discover(config: &Config) -> Result<IpAddr> {
    let output = Command::new("ip")
        .args(["-j", "neigh", "show", "dev", &config.hotspot_interface])
        .output()
        .context("run ip neigh")?;
    ensure!(output.status.success(), "ip neigh failed");

    let neighbours: Value = serde_json::from_slice(&output.stdout)?;
    let mac = config.target_mac.to_ascii_lowercase();
    neighbours
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry["lladdr"]
                .as_str()
                .is_some_and(|lladdr| lladdr.eq_ignore_ascii_case(&mac))
        })
        .find_map(|entry| entry["dst"].as_str()?.parse::<IpAddr>().ok())
        .with_context(|| {
            format!(
                "feeder not in {}'s neighbour table; is it joined? Pass --host",
                config.hotspot_interface
            )
        })
}

/// Stream `file` (or `-` for stdin) to `host`, at `rate` Hz.
#[allow(clippy::print_stdout)]
pub fn play(config: &Config, file: &Path, host: IpAddr, rate: u32) -> Result<()> {
    ensure!(
        RATES.contains(&rate),
        "rate must be one of {RATES:?} (the codec's rates)"
    );
    let key = config
        .firmware
        .as_ref()
        .and_then(|firmware| firmware.request_key())
        .context("no firmware request key in the config (wellfed-local firmware-request-key)")?;

    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(host, PORT), CONNECT_TIMEOUT)
        .with_context(|| format!("connect to {host}:{PORT}; is the speaker variant installed?"))?;
    stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_nodelay(true)?;

    let mut hello = [0u8; HELLO_LEN];
    stream
        .read_exact(&mut hello)
        .context("read the feeder's hello")?;
    let challenge = speaker_stream::challenge(&hello).context("not a PLSPK1 feeder")?;
    let header: [u8; HEADER_LEN] = speaker_stream::header(key.bytes(), &challenge, rate);
    stream.write_all(&header)?;

    let mut status = [0u8; 1];
    stream
        .read_exact(&mut status)
        .context("read the feeder's answer")?;
    match Status::from_byte(status[0]) {
        Some(Status::Ok) => {}
        Some(refused) => bail!("feeder refused the stream: {}", refused.name()),
        None => bail!("feeder sent an unknown status {}", status[0]),
    }

    let input = if file == Path::new("-") {
        "pipe:0".to_owned()
    } else {
        ensure!(file.exists(), "{} does not exist", file.display());
        file.display().to_string()
    };
    let mut decoder = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i", &input])
        .args(["-f", "s16le", "-acodec", "pcm_s16le", "-ac", "1", "-ar"])
        .arg(rate.to_string())
        .arg("pipe:1")
        .stdin(if input == "pipe:0" {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .spawn()
        .context("start ffmpeg (is it installed?)")?;
    let mut pcm = decoder.stdout.take().context("ffmpeg output")?;

    println!("Streaming {} to {host} at {rate} Hz mono…", file.display());
    let begin = Instant::now();
    let mut buffer = vec![0u8; 8192];
    let mut sent = 0u64;
    loop {
        let n = pcm.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        stream
            .write_all(&buffer[..n])
            .context("the feeder closed the stream")?;
        sent += n as u64;
    }
    stream.shutdown(std::net::Shutdown::Write)?;
    ensure!(
        decoder.wait()?.success(),
        "ffmpeg failed to decode the file"
    );

    // The feeder closes once its buffer has played out.
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let _ = stream.read(&mut status);
    let seconds = sent as f64 / f64::from(rate * 2);
    println!(
        "Sent {seconds:.1} s of audio in {:.1} s.",
        begin.elapsed().as_secs_f64()
    );
    Ok(())
}
