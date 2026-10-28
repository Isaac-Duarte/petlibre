//! The Wellfed Bench relay port: SH-UR04A `AT+CHn` emulation plus telemetry.
//!
//! `wellfed-relay` drives the board exactly like the DSD TECH SH-UR04A it
//! replaces: it flushes input, sends one `AT+CHn=0|1|?` line and requires the
//! exact reply `OK+CHn=v`, so those replies are live interop and stay
//! byte-exact. Everything here is pure so it runs in host tests; the firmware
//! (`wellfed-bench-board`) owns the pins and feeds lines in.

#![cfg_attr(not(test), no_std)]

use core::fmt::Write;

/// Longest accepted command line. Longer lines are answered with `ERROR`.
pub const LINE_MAX: usize = 32;

/// Reply buffer size; `AT+STAT?` is the longest reply.
pub const REPLY_MAX: usize = 128;

pub type Reply = heapless::String<REPLY_MAX>;

/// The four emulated relays. The discriminant is the SH-UR04A channel number
/// the bench sends, so it is fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    /// CH1: feeder USB 5 V (AP2171W enable).
    Power = 1,
    /// CH2: feeder TX held to GND through 4.7 kΩ, the loader-entry strap.
    Strap = 2,
    /// CH3: adapter TXD to feeder RX (74LVC2G125 channel A).
    UartTx = 3,
    /// CH4: feeder TX to adapter RXD (74LVC2G125 channel B).
    UartRx = 4,
}

impl Channel {
    pub const ALL: [Self; 4] = [Self::Power, Self::Strap, Self::UartTx, Self::UartRx];

    /// The channel a command line names, or `None` outside 1–4.
    pub fn from_number(n: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|channel| channel.number() == n)
    }

    pub fn number(self) -> u8 {
        self as u8
    }

    /// Bit and array position: CH1 is 0.
    pub fn index(self) -> usize {
        usize::from(self.number() - 1)
    }
}

/// Commanded relay state. All off is the de-energised board: feeder unpowered,
/// strap released, UART isolated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channels([bool; 4]);

impl Channels {
    pub fn get(self, channel: Channel) -> bool {
        self.0[channel.index()]
    }

    pub fn set(&mut self, channel: Channel, on: bool) {
        self.0[channel.index()] = on;
    }
}

/// One parsed relay-port line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `AT`
    Ping,
    /// `AT+CHn=0|1`
    Set(Channel, bool),
    /// `AT+CHn=?`
    Get(Channel),
    /// `AT+VER?`
    Version,
    /// `AT+VBUS?`: feeder 5 V in mV.
    Vbus,
    /// `AT+CUR?`: feeder current in mA.
    Current,
    /// `AT+SRC?`: which USB-C input feeds the feeder.
    Source,
    /// `AT+FLT?`: AP2171W overcurrent flag.
    Fault,
    /// `AT+TXV?`: feeder TX level in mV.
    TxLevel,
    /// `AT+ALIVE?`: feeder MCU powered (its TX idles high).
    Alive,
    /// `AT+STAT?`: everything on one line.
    Status,
    /// `AT+BOOTSEL`: reboot into the RP2040 UF2 bootloader.
    Bootsel,
}

/// Parses one line without its terminator. Case-insensitive, surrounding
/// whitespace and NULs ignored.
pub fn parse(line: &[u8]) -> Option<Command> {
    let line = trim(line);
    let mut upper = heapless::Vec::<u8, LINE_MAX>::new();
    for &b in line {
        upper.push(b.to_ascii_uppercase()).ok()?;
    }
    let rest = upper.strip_prefix(b"AT")?;
    if rest.is_empty() {
        return Some(Command::Ping);
    }
    let rest = rest.strip_prefix(b"+")?;

    if let Some(ch) = rest.strip_prefix(b"CH") {
        let (&n, value) = ch.split_first()?;
        let channel = Channel::from_number(n.checked_sub(b'0')?)?;
        return match value {
            b"=0" => Some(Command::Set(channel, false)),
            b"=1" => Some(Command::Set(channel, true)),
            b"=?" => Some(Command::Get(channel)),
            _ => None,
        };
    }

    Some(match rest {
        b"VER?" => Command::Version,
        b"VBUS?" => Command::Vbus,
        b"CUR?" => Command::Current,
        b"SRC?" => Command::Source,
        b"FLT?" => Command::Fault,
        b"TXV?" => Command::TxLevel,
        b"ALIVE?" => Command::Alive,
        b"STAT?" => Command::Status,
        b"BOOTSEL" => Command::Bootsel,
        _ => return None,
    })
}

fn trim(raw: &[u8]) -> &[u8] {
    let junk = |b: &u8| matches!(b, b'\0' | b' ' | b'\t' | b'\r' | b'\n');
    let start = raw.iter().position(|b| !junk(b)).unwrap_or(raw.len());
    let end = raw.iter().rposition(|b| !junk(b)).map_or(start, |i| i + 1);
    &raw[start..end]
}

/// Collects bytes into lines ended by CR or LF. A CRLF pair yields one line
/// and blank lines are skipped, so the SH-UR04A's `\r\n` framing and bare
/// terminal input both work.
#[derive(Default)]
pub struct LineBuffer {
    buf: heapless::Vec<u8, LINE_MAX>,
    overflow: bool,
}

/// What [`LineBuffer::push`] yields at a terminator.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Complete(heapless::Vec<u8, LINE_MAX>),
    /// The line was longer than [`LINE_MAX`]; it is discarded.
    TooLong,
}

impl LineBuffer {
    pub fn push(&mut self, byte: u8) -> Option<Line> {
        if byte == b'\r' || byte == b'\n' {
            let overflow = core::mem::take(&mut self.overflow);
            let line = core::mem::take(&mut self.buf);
            return if overflow {
                Some(Line::TooLong)
            } else if trim(&line).is_empty() {
                None
            } else {
                Some(Line::Complete(line))
            };
        }
        if self.buf.push(byte).is_err() {
            self.overflow = true;
        }
        None
    }
}

/// Which USB-C input the TPS2116 routes to the feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Host,
    Aux,
}

/// Latest measurements, already scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Telemetry {
    pub vbus_mv: u32,
    pub current_ma: u32,
    pub source: Source,
    pub fault: bool,
    pub tx_mv: u32,
}

impl Telemetry {
    pub const fn new() -> Self {
        Self {
            vbus_mv: 0,
            current_ma: 0,
            source: Source::Host,
            fault: false,
            tx_mv: 0,
        }
    }

    /// The feeder's TX idles high whenever its MCU has power. With CH1 off
    /// that means the D-cell batteries are still in.
    pub fn alive(&self) -> bool {
        self.tx_mv >= ALIVE_MV
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

/// Feeder TX above this counts as driven high by a powered MCU.
pub const ALIVE_MV: u32 = 2000;

/// ADC reference (the 3V3 rail) and full scale of the 12-bit converter.
const VREF_MV: u32 = 3300;
const ADC_FULL: u32 = 4095;

/// Converts a raw 12-bit ADC reading to millivolts at the pin.
pub fn adc_mv(raw: u16) -> u32 {
    u32::from(raw.min(4095)) * VREF_MV / ADC_FULL
}

/// VBUS_FDR through R12 10k over R13 15k: V = V_pin × 25 / 15.
pub fn vbus_mv(raw: u16) -> u32 {
    adc_mv(raw) * 25 / 15
}

/// INA181A2 (50 V/V) across R23 50 mΩ: 2.5 V per amp, so mA = mV × 0.4.
pub fn current_ma(raw: u16) -> u32 {
    adc_mv(raw) * 2 / 5
}

/// The board revision `AT+VER?` reports, as printed on its silkscreen.
pub const BOARD_REVISION: &str = "revC";

/// Formats the reply to `command`. `channels` must already reflect a `Set`.
pub fn reply(command: Command, channels: Channels, telemetry: &Telemetry, version: &str) -> Reply {
    let mut out = Reply::new();
    // Every reply fits REPLY_MAX; a write error would only truncate it.
    let _ = match command {
        Command::Ping | Command::Bootsel => out.write_str("OK"),
        Command::Set(channel, _) | Command::Get(channel) => write!(
            out,
            "OK+CH{}={}",
            channel.number(),
            u8::from(channels.get(channel))
        ),
        Command::Version => write!(out, "OK+VER={version} {BOARD_REVISION}"),
        Command::Vbus => write!(out, "OK+VBUS={}", telemetry.vbus_mv),
        Command::Current => write!(out, "OK+CUR={}", telemetry.current_ma),
        Command::Source => write!(out, "OK+SRC={}", source_name(telemetry.source)),
        Command::Fault => write!(out, "OK+FLT={}", u8::from(telemetry.fault)),
        Command::TxLevel => write!(out, "OK+TXV={}", telemetry.tx_mv),
        Command::Alive => write!(out, "OK+ALIVE={}", u8::from(telemetry.alive())),
        Command::Status => {
            let ch = |c| u8::from(channels.get(c));
            write!(
                out,
                "OK+STAT=CH:{}{}{}{};VBUS:{};CUR:{};SRC:{};FLT:{};TXV:{};ALIVE:{}",
                ch(Channel::Power),
                ch(Channel::Strap),
                ch(Channel::UartTx),
                ch(Channel::UartRx),
                telemetry.vbus_mv,
                telemetry.current_ma,
                source_name(telemetry.source),
                u8::from(telemetry.fault),
                telemetry.tx_mv,
                u8::from(telemetry.alive()),
            )
        }
    };
    out
}

/// Reply to a line that did not parse or was too long.
pub const ERROR: &str = "ERROR";

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Host => "HOST",
        Source::Aux => "AUX",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Vec<Line> {
        let mut buf = LineBuffer::default();
        input.iter().filter_map(|&b| buf.push(b)).collect()
    }

    fn complete(s: &str) -> Line {
        Line::Complete(heapless::Vec::from_slice(s.as_bytes()).unwrap())
    }

    #[test]
    fn bench_set_and_query_get_exact_replies() {
        // The strings wellfed-relay requires, byte for byte.
        let mut channels = Channels::default();
        let t = Telemetry::new();

        let cmd = parse(b"AT+CH1=1").unwrap();
        assert_eq!(cmd, Command::Set(Channel::Power, true));
        if let Command::Set(c, on) = cmd {
            channels.set(c, on);
        }
        assert_eq!(reply(cmd, channels, &t, "0.1.0"), "OK+CH1=1");

        let q1 = parse(b"AT+CH1=?").unwrap();
        assert_eq!(reply(q1, channels, &t, "0.1.0"), "OK+CH1=1");
        let q2 = parse(b"AT+CH2=?").unwrap();
        assert_eq!(reply(q2, channels, &t, "0.1.0"), "OK+CH2=0");

        let off = parse(b"AT+CH4=0").unwrap();
        assert_eq!(reply(off, channels, &t, "0.1.0"), "OK+CH4=0");
    }

    #[test]
    fn all_four_channels_map_in_order() {
        for n in 1..=4u8 {
            let line = format!("AT+CH{n}=?");
            let Some(Command::Get(c)) = parse(line.as_bytes()) else {
                panic!("{line}");
            };
            assert_eq!(c.number(), n);
            assert_eq!(c.index(), usize::from(n - 1));
        }
        assert_eq!(parse(b"AT+CH1=1"), Some(Command::Set(Channel::Power, true)));
        assert_eq!(parse(b"AT+CH2=1"), Some(Command::Set(Channel::Strap, true)));
        assert_eq!(
            parse(b"AT+CH3=1"),
            Some(Command::Set(Channel::UartTx, true))
        );
        assert_eq!(
            parse(b"AT+CH4=1"),
            Some(Command::Set(Channel::UartRx, true))
        );
    }

    #[test]
    fn bad_commands_are_refused() {
        for bad in [
            &b"AT+CH0=1"[..],
            b"AT+CH5=1",
            b"AT+CH1=2",
            b"AT+CH1",
            b"AT+CH12=1",
            b"AT+CH1=1x",
            b"AT+",
            b"ATZ",
            b"HELLO",
            b"",
        ] {
            assert_eq!(parse(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn case_whitespace_and_nul_are_tolerated() {
        assert_eq!(
            parse(b"\0 at+ch3=1 \0"),
            Some(Command::Set(Channel::UartTx, true))
        );
        assert_eq!(parse(b"at"), Some(Command::Ping));
        assert_eq!(parse(b"at+stat?"), Some(Command::Status));
    }

    #[test]
    fn crlf_cr_and_lf_each_end_one_line() {
        assert_eq!(lines(b"AT+CH1=1\r\n"), [complete("AT+CH1=1")]);
        assert_eq!(
            lines(b"AT\rAT+CH2=?\r"),
            [complete("AT"), complete("AT+CH2=?")]
        );
        assert_eq!(lines(b"AT\nAT\n"), [complete("AT"), complete("AT")]);
        assert_eq!(lines(b"\r\n\r\n"), []);
    }

    #[test]
    fn overlong_lines_are_dropped_and_the_next_one_works() {
        let mut input = vec![b'A'; LINE_MAX + 5];
        input.extend_from_slice(b"\r\nAT\r\n");
        assert_eq!(lines(&input), [Line::TooLong, complete("AT")]);
    }

    #[test]
    fn scaling_matches_the_dividers() {
        // 5.0 V on VBUS_FDR puts 3.0 V on the pin.
        let raw_3v0 = (3000 * 4095 / 3300) as u16;
        assert!((4990..=5000).contains(&vbus_mv(raw_3v0)));
        // 1 A gives 2.5 V out of the INA181A2.
        let raw_2v5 = (2500 * 4095 / 3300) as u16;
        assert!((995..=1000).contains(&current_ma(raw_2v5)));
        assert_eq!(adc_mv(4095), 3300);
        assert_eq!(adc_mv(u16::MAX), 3300);
        assert_eq!(adc_mv(0), 0);
    }

    #[test]
    fn telemetry_replies() {
        let t = Telemetry {
            vbus_mv: 5012,
            current_ma: 123,
            source: Source::Aux,
            fault: false,
            tx_mv: 3290,
        };
        let mut ch = Channels::default();
        ch.set(Channel::Power, true);
        ch.set(Channel::UartRx, true);
        let r = |cmd| reply(cmd, ch, &t, "0.1.0");
        assert_eq!(r(Command::Ping), "OK");
        assert_eq!(r(Command::Version), "OK+VER=0.1.0 revC");
        assert_eq!(r(Command::Vbus), "OK+VBUS=5012");
        assert_eq!(r(Command::Current), "OK+CUR=123");
        assert_eq!(r(Command::Source), "OK+SRC=AUX");
        assert_eq!(r(Command::Fault), "OK+FLT=0");
        assert_eq!(r(Command::TxLevel), "OK+TXV=3290");
        assert_eq!(r(Command::Alive), "OK+ALIVE=1");
        assert_eq!(
            r(Command::Status),
            "OK+STAT=CH:1001;VBUS:5012;CUR:123;SRC:AUX;FLT:0;TXV:3290;ALIVE:1"
        );
    }

    #[test]
    fn worst_case_status_fits() {
        let t = Telemetry {
            vbus_mv: u32::MAX,
            current_ma: u32::MAX,
            source: Source::Host,
            fault: true,
            tx_mv: u32::MAX,
        };
        let r = reply(Command::Status, Channels([true; 4]), &t, "0.1.0");
        assert!(r.ends_with("ALIVE:1"), "{r}");
    }
}
