//! Bounded properties of the device protocol's codec: every message a
//! client or feeder can be handed decodes or is refused without a panic,
//! whatever a decoded message re-encodes to reads back as the same value,
//! and every request a client can build survives the trip to the feeder.
//! The byte fixtures (`fixtures.rs`) pin the encoding itself; these
//! properties cover the values between them.

use proptest::prelude::*;
use wellfed_proto::api::*;
use wellfed_proto::setup::{self, Answer, Reply, Request};
use wellfed_proto::{Error, decode, decode_padded, encode, trace};

/// A string for a borrowed field. Tests only: each case leaks a few bytes
/// so a generated value can hold `&'static` fields.
fn text(max: usize) -> impl Strategy<Value = &'static str> {
    proptest::collection::vec(any::<char>(), 0..max)
        .prop_map(|chars| &*Box::leak(chars.into_iter().collect::<String>().into_boxed_str()))
}

fn bytes(max: usize) -> impl Strategy<Value = &'static [u8]> {
    proptest::collection::vec(any::<u8>(), 0..max)
        .prop_map(|bytes| &*Box::leak(bytes.into_boxed_slice()))
}

fn wifi() -> impl Strategy<Value = Wifi<'static>> {
    (bytes(40), text(70)).prop_map(|(ssid, psk)| Wifi { ssid, psk })
}

fn role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Owner),
        Just(Role::Member),
        Just(Role::Server),
        Just(Role::UpdateSigner),
    ]
}

fn tune() -> impl Strategy<Value = Tune> {
    prop_oneof![
        Just(Tune::Chime),
        Just(Tune::Meal),
        Just(Tune::Coin),
        Just(Tune::Alert),
        Just(Tune::Test),
        Just(Tune::Twinkle),
    ]
}

fn window() -> impl Strategy<Value = Window> {
    prop_oneof![
        Just(Window::Always),
        (any::<u16>(), any::<u16>()).prop_map(|(start, end)| Window::Between { start, end }),
    ]
}

fn meal_reminder() -> impl Strategy<Value = MealReminder> {
    prop_oneof![
        Just(MealReminder::Off),
        any::<u8>().prop_map(|minutes| MealReminder::Before { minutes }),
    ]
}

fn op() -> impl Strategy<Value = Op<'static>> {
    let changes = (
        any::<Option<bool>>(),
        any::<Option<bool>>(),
        any::<Option<u8>>(),
        proptest::option::of(window()),
        proptest::option::of(window()),
        any::<Option<u8>>(),
        proptest::option::of(meal_reminder()),
    )
        .prop_map(
            |(
                lights,
                sound,
                volume,
                lights_window,
                sound_window,
                desiccant_days,
                meal_reminder,
            )| {
                SettingsChanges {
                    lights,
                    sound,
                    volume,
                    lights_window,
                    sound_window,
                    desiccant_days,
                    meal_reminder,
                }
            },
        );
    let sync = (any::<u64>(), any::<i32>(), any::<u64>(), any::<i32>()).prop_map(
        |(unix_ms, offset_s, next_change_s, next_offset_s)| TimeSync {
            unix_ms,
            offset_s,
            next_change_s,
            next_offset_s,
        },
    );
    prop_oneof![
        Just(Op::Status),
        Just(Op::GetSettings),
        (any::<u32>(), changes).prop_map(|(base, changes)| Op::PutSettings { base, changes }),
        (any::<u32>(), proptest::option::of(wifi()))
            .prop_map(|(base, wifi)| Op::PutWifi { base, wifi }),
        any::<bool>().prop_map(|locked| Op::SetLock { locked }),
        tune().prop_map(|tune| Op::Play { tune }),
        sync.prop_map(Op::TimeSync),
        role().prop_map(|role| Op::CreateInvite { role }),
        Just(Op::ListClients),
        any::<[u8; 32]>().prop_map(|key| Op::RevokeClient { key }),
        any::<u8>().prop_map(|portions| Op::Feed { portions }),
        Just(Op::Reboot),
        any::<u32>().prop_map(|feed| Op::CancelFeed { feed }),
        (bytes(200), proptest::option::of(bytes(200))).prop_map(|(manifest, downgrade)| {
            Op::OfferUpdate {
                manifest,
                downgrade,
            }
        }),
        (any::<u32>(), bytes(UPDATE_CHUNK_MAX + 1))
            .prop_map(|(offset, data)| Op::UpdateChunk { offset, data }),
        Just(Op::UpdateState),
        Just(Op::CancelUpdate),
        (any::<[u8; 32]>(), text(24)).prop_map(|(key, label)| Op::EnrollUpdateKey { key, label }),
        bytes(200).prop_map(|statement| Op::UpdateKeyStatement { statement }),
        Just(Op::GetTime),
        (
            any::<u32>(),
            proptest::option::of(text(60)),
            proptest::option::of(ntp())
        )
            .prop_map(|(base, zone, ntp)| Op::PutTime {
                base,
                changes: TimeChanges { zone, ntp },
            }),
        any::<Option<u32>>().prop_map(|after| Op::TailLog { after }),
        (0..StatsSection::ALL.len()).prop_map(|index| Op::Stats {
            section: StatsSection::ALL[index],
        }),
        (any::<u32>(), any::<u16>()).prop_map(|(feed, offset)| Op::FeedTrace { feed, offset }),
        Just(Op::GetPlans),
        (any::<u32>(), plans()).prop_map(|(base, plans)| Op::PutPlans { base, plans }),
        Just(Op::ResetDesiccant),
        Just(Op::ReadStockPlans),
        Just(Op::GetCall),
        (any::<u32>(), any::<u32>()).prop_map(|(bytes, crc)| Op::StartCall { bytes, crc }),
        (any::<u32>(), bytes(CALL_CHUNK_MAX + 1))
            .prop_map(|(offset, data)| Op::CallChunk { offset, data }),
        Just(Op::DeleteCall),
    ]
}

fn plans() -> impl Strategy<Value = [Option<Plan>; PLANS_MAX]> {
    let plan = (
        any::<u16>(),
        any::<u8>(),
        any::<u8>(),
        any::<bool>(),
        any::<u8>(),
    )
        .prop_map(|(minute, portions, days, enabled, call)| Plan {
            minute,
            portions,
            days,
            enabled,
            call,
        });
    proptest::array::uniform10(proptest::option::of(plan))
}

/// One trace entry, timed from `at_ms`, at most `step` ms after it.
fn trace_entry(step: u32) -> impl Strategy<Value = (u32, trace::Entry)> {
    use trace::{Entry, Motor};
    let motor = prop_oneof![Just(Motor::Stop), Just(Motor::Drive0), Just(Motor::Drive1)];
    (
        0..step,
        prop_oneof![
            8 => (0u32..5_000).prop_map(|value| Entry::Current { at_ms: 0, value }),
            1 => (1u32..300, any::<bool>()).prop_map(|(edges, high)| Entry::Mark { at_ms: 0, edges, high }),
            1 => (any::<bool>(), any::<bool>())
                .prop_map(|(outlet_clear, food)| Entry::Beams { at_ms: 0, outlet_clear, food }),
            1 => (0u32..6_000).prop_map(|mv| Entry::Battery { at_ms: 0, mv }),
            1 => motor.prop_map(|motor| Entry::Motor { at_ms: 0, motor }),
        ],
    )
}

fn retimed(entry: trace::Entry, at_ms: u32) -> trace::Entry {
    use trace::Entry;
    match entry {
        Entry::Current { value, .. } => Entry::Current { at_ms, value },
        Entry::Mark { edges, high, .. } => Entry::Mark { at_ms, edges, high },
        Entry::Beams {
            outlet_clear, food, ..
        } => Entry::Beams {
            at_ms,
            outlet_clear,
            food,
        },
        Entry::Battery { mv, .. } => Entry::Battery { at_ms, mv },
        Entry::Motor { motor, .. } => Entry::Motor { at_ms, motor },
    }
}

fn ntp() -> impl Strategy<Value = NtpServer> {
    prop_oneof![
        Just(NtpServer::Off),
        any::<u16>().prop_map(|port| NtpServer::Gateway { port }),
        (any::<[u8; 4]>(), any::<u16>()).prop_map(|(ip, port)| NtpServer::Address { ip, port }),
    ]
}

fn client_message() -> impl Strategy<Value = ClientMessage<'static>> {
    prop_oneof![
        (any::<RequestId>(), op()).prop_map(|(id, op)| ClientMessage::Request { id, op }),
        (any::<Option<u64>>(), any::<bool>())
            .prop_map(|(after, samples)| ClientMessage::Subscribe { after, samples }),
    ]
}

fn open() -> impl Strategy<Value = Open> {
    prop_oneof![
        Just(Open::Session),
        any::<u32>().prop_map(|id| Open::Invite { id }),
        Just(Open::Recovery),
    ]
}

fn answer() -> impl Strategy<Value = Answer<'static>> {
    prop_oneof![
        (any::<Option<[u8; 32]>>(), any::<Option<[u8; 32]>>())
            .prop_map(|(open, commit)| Answer::Round { open, commit }),
        (any::<[u8; 32]>(), proptest::option::of(wifi()))
            .prop_map(|(open, wifi)| Answer::Last { open, wifi }),
    ]
}

fn reply() -> impl Strategy<Value = Reply> {
    let refusal = proptest::sample::select(vec![
        setup::Refusal::Busy,
        setup::Refusal::Unconfirmed,
        setup::Refusal::WrongCode,
        setup::Refusal::Full,
        setup::Refusal::NotWritable,
        setup::Refusal::Invalid,
    ]);
    let enrolled = prop_oneof![Just(setup::Enrolled::New), Just(setup::Enrolled::Already)];
    let saved = prop_oneof![
        Just(setup::WifiSaved::Saved),
        Just(setup::WifiSaved::Unchanged)
    ];
    prop_oneof![
        any::<u16>().prop_map(|seconds| Reply::EnterCode { seconds }),
        (any::<Option<[u8; 32]>>(), any::<Option<[u8; 32]>>())
            .prop_map(|(open, commit)| Reply::Round { open, commit }),
        (enrolled, saved).prop_map(|(enrolled, wifi)| Reply::Done { enrolled, wifi }),
        refusal.prop_map(Reply::Refused),
    ]
}

/// Encode `value` and decode it back: the same value, from exactly the
/// encoded bytes. A shorter buffer is refused, never overrun.
macro_rules! round_trip {
    ($ty:ty, $value:expr) => {{
        let value = $value;
        let mut out = [0u8; FRAME_MAX];
        let len = encode(&value, &mut out).unwrap();
        prop_assert_eq!(decode::<$ty>(&out[..len]), Ok(value));
        if len > 0 {
            prop_assert_eq!(
                encode(&value, &mut out[..len - 1]),
                Err(Error::BufferTooSmall)
            );
        }
    }};
}

/// Decode any bytes as `$ty`; whatever decodes re-encodes to bytes that
/// decode to the same value.
macro_rules! stable {
    ($ty:ty, $bytes:expr) => {{
        if let Ok(value) = decode::<$ty>($bytes) {
            let mut out = [0u8; 4 * FRAME_MAX];
            let len = encode(&value, &mut out).unwrap();
            prop_assert_eq!(decode::<$ty>(&out[..len]), Ok(value));
        }
    }};
}

/// Arbitrary bytes, or a valid variant tag followed by arbitrary bytes, so
/// the cases reach past the first byte.
fn wire() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..64),
        (0u8..16, proptest::collection::vec(any::<u8>(), 0..96)).prop_map(|(tag, mut rest)| {
            rest.insert(0, tag);
            rest
        }),
    ]
}

proptest! {
    #[test]
    fn every_client_message_round_trips(message in client_message()) {
        round_trip!(ClientMessage<'_>, message);
    }

    #[test]
    fn opening_messages_round_trip(
        open in open(),
        major in any::<u16>(),
        capabilities in any::<u32>(),
        label in text(24),
        role in role(),
        build in text(40),
        epoch in any::<u32>(),
    ) {
        round_trip!(Open, open);
        round_trip!(Hello, Hello { major, capabilities: Capabilities(capabilities) });
        round_trip!(Join<'_>, Join { label });
        round_trip!(Joined, Joined::Enrolled { role });
        round_trip!(
            Welcome<'_>,
            Welcome::Open { major, capabilities: Capabilities(capabilities), build, role, epoch }
        );
    }

    #[test]
    fn setup_messages_round_trip(label in text(24), answer in answer(), reply in reply()) {
        round_trip!(Request<'_>, Request { label });
        round_trip!(Answer<'_>, answer);
        let mut out = [0u8; setup::REPLY_MAX];
        let len = encode(&reply, &mut out).unwrap();
        prop_assert_eq!(decode::<Reply>(&out[..len]), Ok(reply));
    }

    /// What the feeder decodes from a peer: the clear opening frame, the
    /// handshake payloads, and each sealed request.
    #[test]
    fn feeder_side_decoders_survive_any_bytes(bytes in wire()) {
        stable!(Open, &bytes);
        stable!(Hello, &bytes);
        stable!(Join<'_>, &bytes);
        stable!(ClientMessage<'_>, &bytes);
        stable!(Request<'_>, &bytes);
        stable!(Answer<'_>, &bytes);
    }

    /// What a client decodes from a feeder, or from anything answering on
    /// its address.
    #[test]
    fn client_side_decoders_survive_any_bytes(bytes in wire()) {
        stable!(Welcome<'_>, &bytes);
        stable!(Joined, &bytes);
        stable!(FeederMessage<'_>, &bytes);
        stable!(Reply, &bytes);
    }

    /// A message is exactly its bytes: one more trailing byte is refused,
    /// and an event body reads back from its zero-padded record field.
    #[test]
    fn trailing_bytes_are_refused(message in client_message(), extra in any::<u8>()) {
        let mut out = [0u8; FRAME_MAX];
        let len = encode(&message, &mut out).unwrap();
        out[len] = extra;
        prop_assert_eq!(decode::<ClientMessage<'_>>(&out[..=len]), Err(Error::Malformed));
    }

    /// A body built as the feeder builds one (each entry pushed, halving
    /// while it is full, ending at the first entry nothing makes room for)
    /// reads back whole: every mark, beam, battery and motor entry pushed
    /// before the end, in order and on time, and the current readings a
    /// subsequence of those pushed.
    #[test]
    fn a_body_reads_back_through_any_halving(
        steps in proptest::collection::vec(trace_entry(400), 0..1_500),
    ) {
        use trace::{Entry, Pushed, Reader, Writer};

        let mut writer = Writer::<{ trace::BODY_MAX }>::new();
        let mut pushed = Vec::new();
        let mut at_ms = 0u32;
        for (step, entry) in steps {
            at_ms += step;
            let entry = retimed(entry, at_ms);
            let kept = loop {
                match writer.push(entry) {
                    Pushed::Kept => break true,
                    Pushed::Full if writer.halve() => {}
                    Pushed::Full => break false,
                }
            };
            if !kept {
                break;
            }
            pushed.push(entry);
        }

        let read: Vec<Entry> = Reader::new(writer.bytes()).map(Result::unwrap).collect();
        let events = |entries: &[Entry]| -> Vec<Entry> {
            entries.iter().copied().filter(|e| !matches!(e, Entry::Current { .. })).collect()
        };
        prop_assert_eq!(events(&read), events(&pushed));
        let mut currents = pushed.iter().filter(|e| matches!(e, Entry::Current { .. }));
        for entry in read.iter().filter(|e| matches!(e, Entry::Current { .. })) {
            prop_assert!(currents.any(|pushed| pushed == entry));
        }
        prop_assert_eq!(
            u32::try_from(read.iter().filter(|e| matches!(e, Entry::Current { .. })).count()).unwrap(),
            writer.currents()
        );
    }

    /// Any bytes read as a body end in entries or one error, never a panic.
    #[test]
    fn any_bytes_read_as_a_body(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let mut reader = trace::Reader::new(&bytes);
        let mut errors = 0;
        for entry in &mut reader {
            errors += usize::from(entry.is_err());
        }
        prop_assert!(errors <= 1);
    }

    #[test]
    fn padded_decoding_survives_any_record(body in any::<[u8; 16]>()) {
        if let Some(event) = decode_padded::<EventBody>(&body) {
            let mut out = [0u8; 16];
            let len = encode(&event, &mut out).unwrap();
            prop_assert_eq!(&out[..len], &body[..len]);
        }
    }
}
