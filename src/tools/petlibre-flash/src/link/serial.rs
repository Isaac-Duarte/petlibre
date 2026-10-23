//! The physical UART: 115200 8N1, no flow control, exclusive, and DTR/RTS
//! deasserted, as the Python clients open it.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serialport::{SerialPort, TTYPort};

use crate::link::protocol::Port;

pub struct Serial(TTYPort);

impl Serial {
    pub fn open(path: &str) -> crate::Result<Self> {
        let mut port = serialport::new(path, 115_200)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(50))
            .dtr_on_open(false)
            .open_native()
            .map_err(io::Error::from)?;

        port.set_exclusive(true).map_err(io::Error::from)?;
        modem_line(port.write_request_to_send(false))?;
        modem_line(port.write_data_terminal_ready(false))?;
        Ok(Self(port))
    }
}

/// Like pyserial, accept a device without modem lines (EINVAL or ENOTTY, as
/// on a pseudo-terminal); any other failure stops before transmission.
fn modem_line(result: serialport::Result<()>) -> io::Result<()> {
    match result {
        Err(error)
            if error.kind() == serialport::ErrorKind::Unknown
                && [
                    "Invalid argument",
                    "Inappropriate ioctl for device",
                    "Not a typewriter",
                ]
                .contains(&error.description.as_str()) =>
        {
            Ok(())
        }
        other => other.map_err(io::Error::from),
    }
}

impl Port for Serial {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }

    /// Everything, or as much as fits in pyserial's three-second write
    /// timeout; the caller refuses a short count and never resends.
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut written = 0;
        while written < data.len() && Instant::now() < deadline {
            match self.0.write(&data[written..]) {
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error),
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }

    fn in_waiting(&mut self) -> io::Result<usize> {
        Ok(self.0.bytes_to_read().map_err(io::Error::from)? as usize)
    }
}

/// An emulated helper served over a Unix socket (offline tests only;
/// `petlibre-emu`'s `serve`, driven by `tests/emulated.rs`). Each read asks
/// the emulator to run the helper until it has output.
pub struct Emulated(std::os::unix::net::UnixStream);

impl Emulated {
    pub fn connect(path: &std::path::Path) -> crate::Result<Self> {
        Ok(Self(std::os::unix::net::UnixStream::connect(path)?))
    }

    fn call(&mut self, op: u8, argument: usize, data: &[u8]) -> io::Result<Vec<u8>> {
        let argument = u32::try_from(argument).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "Emulator request too large")
        })?;
        let mut request = vec![op];
        request.extend_from_slice(&argument.to_le_bytes());
        request.extend_from_slice(data);
        self.0.write_all(&request)?;

        let mut length = [0u8; 4];
        self.0.read_exact(&mut length)?;
        let mut reply = vec![0; u32::from_le_bytes(length) as usize];
        self.0.read_exact(&mut reply)?;
        Ok(reply)
    }

    fn number(reply: &[u8]) -> io::Result<usize> {
        let bytes: [u8; 4] = reply
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "malformed emulator reply"))?;
        Ok(u32::from_le_bytes(bytes) as usize)
    }
}

impl Port for Emulated {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let reply = self.call(b'R', buffer.len(), &[])?;
        if reply.len() > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "emulator returned too many bytes",
            ));
        }
        buffer[..reply.len()].copy_from_slice(&reply);
        Ok(reply.len())
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Self::number(&self.call(b'W', data.len(), data)?)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.call(b'F', 0, &[]).map(drop)
    }

    fn in_waiting(&mut self) -> io::Result<usize> {
        Self::number(&self.call(b'I', 0, &[])?)
    }
}
