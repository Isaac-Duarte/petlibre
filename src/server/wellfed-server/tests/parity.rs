//! App parity: every device operation the service gives a route is a route
//! the API document serves, with that method.

use wellfed_model::{
    Bowl, Calibration, ClientRole, Generation, Op, PLANS, SettingsPatch, StatsSection, TimeChanges,
    TimeSync, Tune,
};
use wellfed_service::{Method, Parity, parity};

/// One operation of each kind. The match in [`kind`] has no wildcard, so
/// a new operation does not build until it is given a sample here.
fn every_op() -> Vec<Op> {
    let sync = TimeSync {
        unix_ms: 0,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    let ops = vec![
        Op::Status,
        Op::GetSettings,
        Op::PutSettings {
            base: Generation(0),
            changes: SettingsPatch::default(),
        },
        Op::PutWifi {
            base: Generation(0),
            wifi: None,
        },
        Op::SetLock { locked: true },
        Op::Play { tune: Tune::Chime },
        Op::TimeSync(sync),
        Op::CreateInvite {
            role: ClientRole::Member,
        },
        Op::ListClients,
        Op::RevokeClient { key: [0; 32] },
        Op::Feed { portions: 1 },
        Op::Reboot,
        Op::CancelFeed { feed: 1 },
        Op::OfferUpdate {
            manifest: Vec::new(),
            downgrade: None,
        },
        Op::UpdateChunk {
            offset: 0,
            data: Vec::new(),
        },
        Op::UpdateState,
        Op::CancelUpdate,
        Op::EnrollUpdateKey {
            key: [0; 32],
            label: String::new(),
        },
        Op::UpdateKeyStatement {
            statement: Vec::new(),
        },
        Op::GetTime,
        Op::PutTime {
            base: Generation(0),
            changes: TimeChanges::default(),
        },
        Op::TailLog { after: None },
        Op::Stats {
            section: StatsSection::Counters,
        },
        Op::FeedTrace { feed: 1, offset: 0 },
        Op::GetCalibration,
        Op::PutCalibration {
            base: Generation(0),
            calibration: Calibration {
                portion_mg: None,
                bowl: Bowl::Single,
            },
        },
        Op::ReadBank1 { offset: 0 },
        Op::GetPlans,
        Op::PutPlans {
            base: Generation(0),
            plans: [None; PLANS],
        },
        Op::ResetDesiccant,
        Op::GetCall,
        Op::StartCall { bytes: 1, crc: 0 },
        Op::CallChunk {
            offset: 0,
            data: vec![0],
        },
        Op::DeleteCall,
    ];
    let mut kinds: Vec<_> = ops.iter().map(kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), ops.len(), "one sample of each operation");
    ops
}

/// The operation's kind, by an exhaustive match.
fn kind(op: &Op) -> u8 {
    match op {
        Op::Status => 0,
        Op::GetSettings => 1,
        Op::PutSettings { .. } => 2,
        Op::PutWifi { .. } => 3,
        Op::SetLock { .. } => 4,
        Op::Play { .. } => 5,
        Op::TimeSync(_) => 6,
        Op::CreateInvite { .. } => 7,
        Op::ListClients => 8,
        Op::RevokeClient { .. } => 9,
        Op::Feed { .. } => 10,
        Op::Reboot => 11,
        Op::CancelFeed { .. } => 12,
        Op::OfferUpdate { .. } => 13,
        Op::UpdateChunk { .. } => 14,
        Op::UpdateState => 15,
        Op::CancelUpdate => 16,
        Op::EnrollUpdateKey { .. } => 17,
        Op::UpdateKeyStatement { .. } => 18,
        Op::GetTime => 19,
        Op::PutTime { .. } => 20,
        Op::TailLog { .. } => 21,
        Op::Stats { .. } => 22,
        Op::FeedTrace { .. } => 23,
        Op::GetCalibration => 24,
        Op::PutCalibration { .. } => 25,
        Op::ReadBank1 { .. } => 26,
        Op::GetPlans => 27,
        Op::PutPlans { .. } => 28,
        Op::ResetDesiccant => 29,
        Op::GetCall => 30,
        Op::StartCall { .. } => 31,
        Op::CallChunk { .. } => 32,
        Op::DeleteCall => 33,
    }
}

#[test]
fn every_routed_operation_is_served_by_the_api_document() {
    let document = wellfed_server::openapi();
    let document = serde_json::to_value(&document).expect("the document serializes");

    for op in every_op() {
        let Parity::Route(method, path) = parity(&op) else {
            continue;
        };
        let method = match method {
            Method::Get => "get",
            Method::Post => "post",
            Method::Put => "put",
            Method::Patch => "patch",
            Method::Delete => "delete",
        };
        let full = format!("{}{path}", wellfed_api::PREFIX);
        let served = document["paths"][&full][method].is_object();
        assert!(
            served,
            "{op:?} is routed to {method} {full}, which is not served"
        );
    }
}
