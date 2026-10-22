//! The physical UART: 115200 8N1, no flow control, exclusive, and DTR/RTS
//! deasserted, as the Python clients open it.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serialport::{SerialPort, TTYPort};

use crate::protocol::Port;

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
        port.write_request_to_send(false).map_err(io::Error::from)?;
        port.write_data_terminal_ready(false)
            .map_err(io::Error::from)?;
        Ok(Self(port))
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
