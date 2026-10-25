//! The MQTT transport codec: any bytes as a broker's stream, decoded packet
//! by packet into a session, as the firmware's health client does.
#![no_main]

use wellfed_core::transport::{Event, Session, decode};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let mut session = Session::connecting(0);
    let mut rest = data;
    let mut now = 0u32;
    while let Ok(Some((packet, used))) = decode(rest) {
        assert!(used > 0 && used <= rest.len());
        // Every shorter prefix asks for more bytes rather than failing.
        assert_eq!(decode(&rest[..used - 1]), Ok(None));
        now += 100;
        let event = session.on_packet_with(
            now,
            packet,
            "wellfed/1/dev/request",
            Some("wellfed/1/dev/ota/data"),
        );
        rest = &rest[used..];
        if matches!(event, Event::Closed(_)) {
            break;
        }
    }
});
