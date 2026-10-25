//! The feeder's own mDNS advertisement (RFC 6762, DNS-SD RFC 6763),
//! sans-IO, for the loop task's one UDP socket.
//!
//! The vendor responder is 33.5 KB of code and starts its own task.
//! The feeder needs only one service, so this answers four records and
//! nothing else:
//! - `PTR _wellfed._tcp.local` → `wellfed-XXXXXXXX._wellfed._tcp.local`
//! - `SRV` of that instance → port, on host `wellfed-XXXXXXXX.local`
//! - `TXT` of that instance → `v=1` and `id=` the identity key in hex
//! - `A wellfed-XXXXXXXX.local` → the station's address
//!
//! and the DNS-SD service enumeration (`_services._dns-sd._udp.local`).
//! `XXXXXXXX` is the identity key's first four bytes, so two feeders on one
//! network have different names without probing, and a client can match
//! the key it paired with. Discovery carries no trust: the session's Noise
//! handshake checks the key.
//!
//! A query naming any of these gets every record in one response: the asked
//! records as answers, the rest as additional records. A query from port
//! 5353 is answered by multicast (or unicast, when it asks with the QU bit);
//! a legacy query from another port is answered by unicast with its id and
//! question echoed, as RFC 6762 section 6.7 asks. Every address change is
//! announced twice, a second apart.

use wellfed_session::hex;

pub const PORT: u16 = 5353;
/// 224.0.0.251.
pub const GROUP: u32 = 0xe000_00fb;
/// The largest packet read or written.
pub const PACKET: usize = 512;

const SERVICE: &[&[u8]] = &[b"_wellfed", b"_tcp", b"local"];
const ENUMERATION: &[&[u8]] = &[b"_services", b"_dns-sd", b"_udp", b"local"];
const LOCAL: &[u8] = b"local";

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// The cache-flush bit, on records only this host answers.
const FLUSH: u16 = 0x8000;
/// In a question: answer by unicast.
const UNICAST: u16 = 0x8000;
/// RFC 6762's recommendations: host records 120 s, the others 75 min.
const TTL_HOST: u32 = 120;
const TTL_OTHER: u32 = 4_500;
/// Legacy unicast answers carry at most this.
const TTL_LEGACY: u32 = 10;
const ANNOUNCE_GAP_MS: u32 = 1_000;

/// "wellfed-" and eight hex digits.
const LABEL: usize = 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Multicast,
    /// The sender's address and port.
    Unicast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub len: usize,
    pub to: Destination,
}

/// Which records a query asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Asked {
    ptr: bool,
    srv: bool,
    txt: bool,
    a: bool,
    enumeration: bool,
    unicast: bool,
}

impl Asked {
    const fn any(self) -> bool {
        self.ptr || self.srv || self.txt || self.a || self.enumeration
    }
}

#[derive(Debug)]
pub struct Responder {
    label: [u8; LABEL],
    /// "id=" and 64 hex digits.
    id: [u8; 67],
    port: u16,
    ip: Option<u32>,
    announcements: u8,
    next_ms: u32,
}

impl Responder {
    pub fn new(identity: &[u8; 32], port: u16) -> Self {
        let mut label = *b"wellfed-00000000";
        let _ = hex::encode(&identity[..4], &mut label[9..]);
        let mut id = [0; 67];
        id[..3].copy_from_slice(b"id=");
        let _ = hex::encode(identity, &mut id[3..]);
        Self {
            label,
            id,
            port,
            ip: None,
            announcements: 0,
            next_ms: 0,
        }
    }

    /// The host label, `wellfed-` and the key's first four bytes in hex.
    pub fn label(&self) -> &str {
        core::str::from_utf8(&self.label).unwrap_or("wellfed")
    }

    /// The station's address now; a change is announced afresh.
    pub fn set_ip(&mut self, ip: Option<u32>, now_ms: u32) {
        if ip != self.ip {
            self.ip = ip;
            self.announcements = if ip.is_some() { 2 } else { 0 };
            self.next_ms = now_ms;
        }
    }

    /// An unsolicited announcement to send by multicast now, if one is due.
    pub fn announce(&mut self, now_ms: u32, out: &mut [u8]) -> Option<usize> {
        if self.announcements == 0 || !crate::deadline_passed(now_ms, self.next_ms) {
            return None;
        }
        self.announcements -= 1;
        self.next_ms = now_ms.wrapping_add(ANNOUNCE_GAP_MS);
        let asked = Asked {
            ptr: true,
            srv: true,
            txt: true,
            a: true,
            ..Asked::default()
        };
        self.write(0, None, asked, false, out)
    }

    /// The answer to one received packet, if it asks about this feeder.
    pub fn query(&self, packet: &[u8], from_port: u16, out: &mut [u8]) -> Option<Reply> {
        self.ip?;
        let header = packet.get(..12)?;
        let id = u16::from_be_bytes([header[0], header[1]]);
        let flags = u16::from_be_bytes([header[2], header[3]]);
        // Responses and anything but a standard query are not ours to answer.
        if flags & 0xf800 != 0 {
            return None;
        }
        let questions = u16::from_be_bytes([header[4], header[5]]);

        let mut asked = Asked::default();
        let mut at = 12;
        let mut first_question = None;
        for _ in 0..questions {
            let start = at;
            let name = Name::read(packet, &mut at)?;
            let fields = packet.get(at..at + 4)?;
            at += 4;
            let qtype = u16::from_be_bytes([fields[0], fields[1]]);
            let qclass = u16::from_be_bytes([fields[2], fields[3]]);
            if qclass & !UNICAST != CLASS_IN && qclass & !UNICAST != TYPE_ANY {
                continue;
            }
            let before = asked;
            self.match_question(&name, qtype, &mut asked);
            if asked != before {
                asked.unicast |= qclass & UNICAST != 0;
                first_question = first_question.or(Some((start, at)));
            }
        }
        if !asked.any() {
            return None;
        }

        let legacy = from_port != PORT;
        let echo = if legacy {
            first_question.and_then(|(start, end)| packet.get(start..end))
        } else {
            None
        };
        let len = self.write(if legacy { id } else { 0 }, echo, asked, legacy, out)?;
        let to = if legacy || asked.unicast {
            Destination::Unicast
        } else {
            Destination::Multicast
        };
        Some(Reply { len, to })
    }

    fn match_question(&self, name: &Name, qtype: u16, asked: &mut Asked) {
        let any = qtype == TYPE_ANY;
        if name.is(SERVICE) && (any || qtype == TYPE_PTR) {
            asked.ptr = true;
        }
        if name.is(ENUMERATION) && (any || qtype == TYPE_PTR) {
            asked.enumeration = true;
        }
        if name.is_instance(&self.label) {
            asked.srv |= any || qtype == TYPE_SRV;
            asked.txt |= any || qtype == TYPE_TXT;
        }
        if name.is_host(&self.label) && (any || qtype == TYPE_A) {
            asked.a = true;
        }
    }

    /// One response: the asked records as answers, the rest of the
    /// service's as additional records.
    fn write(
        &self,
        id: u16,
        question: Option<&[u8]>,
        asked: Asked,
        legacy: bool,
        out: &mut [u8],
    ) -> Option<usize> {
        let ip = self.ip?;
        let mut w = Writer { out, at: 12 };
        if let Some(question) = question {
            w.bytes(question)?;
        }
        let (host_ttl, other_ttl) = if legacy {
            (TTL_LEGACY, TTL_LEGACY)
        } else {
            (TTL_HOST, TTL_OTHER)
        };
        // Legacy resolvers do not know the cache-flush bit.
        let flush = if legacy { 0 } else { FLUSH };

        let mut answers = 0u16;
        let mut additional = 0u16;
        let records = [
            (Record::Ptr, asked.ptr),
            (Record::Enumeration, asked.enumeration),
            (Record::Srv, asked.srv),
            (Record::Txt, asked.txt),
            (Record::A, asked.a),
        ];
        // Answers first, then the rest; the enumeration only when asked.
        let answers_first = records.iter().filter(|(_, answer)| *answer);
        let rest = records
            .iter()
            .filter(|&&(record, answer)| !answer && record != Record::Enumeration);
        for &(record, answer) in answers_first.chain(rest) {
            self.record(&mut w, record, ip, host_ttl, other_ttl, flush)?;
            if answer {
                answers += 1;
            } else {
                additional += 1;
            }
        }

        let len = w.at;
        let header = out.get_mut(..12)?;
        header[..2].copy_from_slice(&id.to_be_bytes());
        header[2..4].copy_from_slice(&0x8400u16.to_be_bytes());
        header[4..6].copy_from_slice(&u16::from(question.is_some()).to_be_bytes());
        header[6..8].copy_from_slice(&answers.to_be_bytes());
        header[8..10].copy_from_slice(&0u16.to_be_bytes());
        header[10..12].copy_from_slice(&additional.to_be_bytes());
        Some(len)
    }

    fn record(
        &self,
        w: &mut Writer<'_>,
        record: Record,
        ip: u32,
        host_ttl: u32,
        other_ttl: u32,
        flush: u16,
    ) -> Option<()> {
        match record {
            Record::Ptr => {
                w.name(&[], SERVICE)?;
                w.head(TYPE_PTR, CLASS_IN, other_ttl)?;
                let mark = w.length()?;
                w.name(&[&self.label], SERVICE)?;
                w.end(mark)
            }
            Record::Enumeration => {
                w.name(&[], ENUMERATION)?;
                w.head(TYPE_PTR, CLASS_IN, other_ttl)?;
                let mark = w.length()?;
                w.name(&[], SERVICE)?;
                w.end(mark)
            }
            Record::Srv => {
                w.name(&[&self.label], SERVICE)?;
                w.head(TYPE_SRV, CLASS_IN | flush, host_ttl)?;
                let mark = w.length()?;
                w.bytes(&[0, 0, 0, 0])?;
                w.bytes(&self.port.to_be_bytes())?;
                w.name(&[&self.label], &[LOCAL])?;
                w.end(mark)
            }
            Record::Txt => {
                w.name(&[&self.label], SERVICE)?;
                w.head(TYPE_TXT, CLASS_IN | flush, other_ttl)?;
                let mark = w.length()?;
                w.text(b"v=1")?;
                w.text(&self.id)?;
                w.end(mark)
            }
            Record::A => {
                w.name(&[&self.label], &[LOCAL])?;
                w.head(TYPE_A, CLASS_IN | flush, host_ttl)?;
                let mark = w.length()?;
                w.bytes(&ip.to_be_bytes())?;
                w.end(mark)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Record {
    Ptr,
    Enumeration,
    Srv,
    Txt,
    A,
}

/// A name read from a packet, its labels lowercased, compression followed.
struct Name {
    bytes: [u8; 128],
    /// Label boundaries: (start, end) into `bytes`.
    labels: [(u8, u8); 8],
    count: usize,
}

impl Name {
    /// Read the name at `*at`, moving `*at` past it in the packet.
    fn read(packet: &[u8], at: &mut usize) -> Option<Self> {
        let mut name = Self {
            bytes: [0; 128],
            labels: [(0, 0); 8],
            count: 0,
        };
        let mut used = 0;
        let mut cursor = *at;
        let mut jumped = false;
        // Bound pointer chains, so a loop in a hostile packet ends.
        for _ in 0..32 {
            let len = *packet.get(cursor)?;
            if len & 0xc0 == 0xc0 {
                let low = *packet.get(cursor + 1)?;
                if !jumped {
                    *at = cursor + 2;
                    jumped = true;
                }
                cursor = usize::from(u16::from_be_bytes([len & 0x3f, low]));
                continue;
            }
            if len == 0 {
                if !jumped {
                    *at = cursor + 1;
                }
                return Some(name);
            }
            let len = usize::from(len);
            let label = packet.get(cursor + 1..cursor + 1 + len)?;
            let out = name.bytes.get_mut(used..used + len)?;
            for (o, &b) in out.iter_mut().zip(label) {
                *o = b.to_ascii_lowercase();
            }
            *name.labels.get_mut(name.count)? =
                (u8::try_from(used).ok()?, u8::try_from(used + len).ok()?);
            name.count += 1;
            used += len;
            cursor += 1 + len;
        }
        None
    }

    fn label(&self, index: usize) -> &[u8] {
        let (start, end) = self.labels[index];
        &self.bytes[usize::from(start)..usize::from(end)]
    }

    fn is(&self, labels: &[&[u8]]) -> bool {
        self.count == labels.len() && labels.iter().enumerate().all(|(i, l)| self.label(i) == *l)
    }

    fn is_instance(&self, label: &[u8]) -> bool {
        self.count == 4 && self.label(0) == label && (1..4).all(|i| self.label(i) == SERVICE[i - 1])
    }

    fn is_host(&self, label: &[u8]) -> bool {
        self.count == 2 && self.label(0) == label && self.label(1) == LOCAL
    }
}

struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, bytes: &[u8]) -> Option<()> {
        self.out
            .get_mut(self.at..self.at + bytes.len())?
            .copy_from_slice(bytes);
        self.at += bytes.len();
        Some(())
    }

    /// `first` labels, then `rest`, then the root.
    fn name(&mut self, first: &[&[u8]], rest: &[&[u8]]) -> Option<()> {
        for label in first.iter().chain(rest) {
            self.bytes(&[u8::try_from(label.len()).ok()?])?;
            self.bytes(label)?;
        }
        self.bytes(&[0])
    }

    fn text(&mut self, text: &[u8]) -> Option<()> {
        self.bytes(&[u8::try_from(text.len()).ok()?])?;
        self.bytes(text)
    }

    fn head(&mut self, rtype: u16, class: u16, ttl: u32) -> Option<()> {
        self.bytes(&rtype.to_be_bytes())?;
        self.bytes(&class.to_be_bytes())?;
        self.bytes(&ttl.to_be_bytes())
    }

    /// Reserve the data length; the offset to fill in at [`Self::end`].
    fn length(&mut self) -> Option<usize> {
        let mark = self.at;
        self.bytes(&[0, 0])?;
        Some(mark)
    }

    fn end(&mut self, mark: usize) -> Option<()> {
        let len = u16::try_from(self.at - mark - 2).ok()?;
        self.out
            .get_mut(mark..mark + 2)?
            .copy_from_slice(&len.to_be_bytes());
        Some(())
    }
}
