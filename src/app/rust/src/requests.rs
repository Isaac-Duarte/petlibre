//! A request from Dart as the device API's ops, and the answer as Dart's
//! types. Each op goes through the session's loop under a fresh id and never
//! twice; only an idempotent chunk the feeder said it was too busy for is
//! sent again, and an unanswered one ends the upload.

use std::net::Ipv4Addr;
use std::str::FromStr;
use std::time::Instant;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokio::sync::{mpsc, oneshot};
use wellfed_client::api::Invite;
use wellfed_client::call::{Clip, Step, Upload, UploadError, UploadFailed};
use wellfed_client::plans::{self, EditError};
use wellfed_client::setup::Credentials;
use wellfed_client::trace::Pages;
use wellfed_model::{
    Answer, Calibration, CalibrationView, CallView, Client, ClientRole, Generation, NtpServer, Op,
    Plan, Plans, Refusal, SettingsPatch, TimeChanges, TimeOfDay, TimeView, Weekday, Weekdays,
};

use crate::api::feeder::{
    CalibrationInfo, CallInfo, ClientView, FeederRefusal, FeederReply, FeederRequest, InviteView,
    NotAsked, NtpView, PlanEdit, PlanView, PlansView, PortionChange, SettingsView, TimeInfo,
    TraceView, UpdateInfo, ZoneChange,
};
use crate::session::Command;

/// How a request's one op ended, short of an answer.
enum Lost {
    Unanswered,
    NotSent,
}

impl From<Lost> for FeederReply {
    fn from(lost: Lost) -> Self {
        match lost {
            Lost::Unanswered => Self::Unanswered,
            Lost::NotSent => Self::NotSent,
        }
    }
}

/// One op through the session's loop. A session that ended before sending it
/// drops the reply; one that sent it answers `None` if it heard nothing.
async fn ask(commands: &mpsc::UnboundedSender<Command>, op: Op) -> Result<Answer, Lost> {
    let (reply, answer) = oneshot::channel();
    commands
        .send(Command::Ask { op, reply })
        .map_err(|_| Lost::NotSent)?;
    answer
        .await
        .map_err(|_| Lost::NotSent)?
        .ok_or(Lost::Unanswered)
}

/// Run `request` over the session behind `commands`, with feeder `identity`.
pub async fn run(
    commands: &mpsc::UnboundedSender<Command>,
    identity: [u8; 32],
    request: FeederRequest,
) -> FeederReply {
    let op = match request {
        FeederRequest::GetSettings => Op::GetSettings,
        FeederRequest::PutSettings { base, changes } => Op::PutSettings {
            base: Generation(base),
            changes: SettingsPatch {
                lights: changes.lights,
                sound: changes.sound,
                volume: changes.volume,
                lights_window: changes.lights_window,
                sound_window: changes.sound_window,
                desiccant_days: changes.desiccant_days,
                meal_reminder: changes.meal_reminder,
            },
        },
        FeederRequest::PutWifi {
            base,
            ssid,
            passphrase,
        } => match Credentials::new(ssid.into_bytes(), passphrase) {
            Ok(credentials) => Op::PutWifi {
                base: Generation(base),
                wifi: Some(credentials.into_wifi()),
            },
            Err(_) => return not_asked(NotAsked::Credentials),
        },
        FeederRequest::ForgetWifi { base } => Op::PutWifi {
            base: Generation(base),
            wifi: None,
        },
        FeederRequest::SetLock { locked } => Op::SetLock { locked },
        FeederRequest::Play { tune } => Op::Play { tune },
        FeederRequest::SyncClock => Op::TimeSync(wellfed_client::time::sync(
            Timestamp::now(),
            &TimeZone::system(),
        )),
        FeederRequest::GetTime => Op::GetTime,
        FeederRequest::PutTime { base, zone, ntp } => match time_changes(zone, ntp) {
            Ok(changes) => Op::PutTime {
                base: Generation(base),
                changes,
            },
            Err(reason) => return not_asked(reason),
        },
        FeederRequest::CreateInvite { role } => {
            return match ask(commands, Op::CreateInvite { role }).await {
                Ok(Answer::Invite(invite)) => FeederReply::Invite {
                    invite: invite_of(invite, identity, role),
                },
                Ok(answer) => reply_of(answer),
                Err(lost) => lost.into(),
            };
        }
        FeederRequest::ListClients => Op::ListClients,
        FeederRequest::RevokeClient { key } => match <[u8; 32]>::try_from(key) {
            Ok(key) => Op::RevokeClient { key },
            Err(_) => return not_asked(NotAsked::BadKey),
        },
        FeederRequest::Reboot => Op::Reboot,
        FeederRequest::ResetDesiccant => Op::ResetDesiccant,
        FeederRequest::Stats { section } => Op::Stats { section },
        FeederRequest::ReadTrace { feed } => return read_trace(commands, feed).await,
        FeederRequest::GetCalibration => Op::GetCalibration,
        FeederRequest::PutCalibration {
            base,
            portion,
            bowl,
        } => match portion_mg(portion) {
            Ok(portion_mg) => Op::PutCalibration {
                base: Generation(base),
                calibration: Calibration { portion_mg, bowl },
            },
            Err(reason) => return not_asked(reason),
        },
        FeederRequest::GetPlans => Op::GetPlans,
        FeederRequest::EditPlans { plans, edit } => {
            let held = plans_of(&plans);
            match edit_of(edit).and_then(|edit| plans::edited(&held, edit).map_err(not_edited)) {
                Ok(plans) => Op::PutPlans {
                    base: held.generation,
                    plans,
                },
                Err(reason) => return not_asked(reason),
            }
        }
        FeederRequest::GetCall => Op::GetCall,
        FeederRequest::UploadCall { wav } => return upload_call(commands, &wav).await,
        FeederRequest::DeleteCall => Op::DeleteCall,
        FeederRequest::UpdateState => Op::UpdateState,
        FeederRequest::CancelUpdate => Op::CancelUpdate,
        FeederRequest::TailLog { after } => Op::TailLog { after },
    };

    match ask(commands, op).await {
        Ok(answer) => reply_of(answer),
        Err(lost) => lost.into(),
    }
}

const fn not_asked(reason: NotAsked) -> FeederReply {
    FeederReply::NotAsked { reason }
}

/// The feed's trace, a page at a time. Reads run again freely, but one with
/// no answer ends the read: the session is going away.
async fn read_trace(commands: &mpsc::UnboundedSender<Command>, feed: u32) -> FeederReply {
    let mut pages = Pages::new(feed);
    loop {
        let answer = match ask(commands, pages.next_op()).await {
            Ok(answer) => answer,
            Err(lost) => return lost.into(),
        };
        if let Answer::Refused(refusal) = answer {
            return FeederReply::Refused {
                refusal: refusal_of(refusal),
            };
        }
        match pages.take(answer) {
            Ok(Some(trace)) => {
                return FeederReply::Trace {
                    trace: TraceView {
                        summary: trace.summary,
                        stored: trace.stored,
                        entries: trace.entries,
                    },
                };
            }
            Ok(None) => {}
            Err(_) => return FeederReply::Unexpected,
        }
    }
}

async fn upload_call(commands: &mpsc::UnboundedSender<Command>, wav: &[u8]) -> FeederReply {
    let Ok(clip) = Clip::from_wav(wav) else {
        return not_asked(NotAsked::BadRecording);
    };
    let mut upload = Upload::new(&clip);
    loop {
        match upload.poll(Instant::now()) {
            Step::Ask(op) => match ask(commands, op).await {
                Ok(answer) => upload.answer(&answer, Instant::now()),
                // Starting over is the owner's call.
                Err(lost) => return lost.into(),
            },
            Step::Wait(wait) => tokio::time::sleep(wait).await,
            Step::Stored(view) => {
                return FeederReply::Call {
                    call: call_of(view),
                };
            }
            Step::Failed(UploadFailed::Upload(UploadError::Refused(refusal))) => {
                return FeederReply::Refused {
                    refusal: refusal_of(refusal),
                };
            }
            Step::Failed(_) => return FeederReply::Unexpected,
        }
    }
}

fn time_changes(zone: Option<ZoneChange>, ntp: Option<String>) -> Result<TimeChanges, NotAsked> {
    let now = Timestamp::now();
    let named = |name: &str| {
        let host = TimeZone::get(name).ok();
        wellfed_client::time::rule(name, now, host.as_ref()).map_err(|_| NotAsked::BadZone)
    };
    let zone = match zone {
        None => None,
        Some(ZoneChange::Clear) => Some(String::new()),
        Some(ZoneChange::Named { name }) => Some(named(&name)?),
        Some(ZoneChange::Phone) => {
            let system = TimeZone::system();
            Some(named(system.iana_name().ok_or(NotAsked::BadZone)?)?)
        }
    };
    let ntp = ntp
        .map(|text| NtpServer::from_str(&text).map_err(|_| NotAsked::BadNtp))
        .transpose()?;
    Ok(TimeChanges { zone, ntp })
}

fn portion_mg(portion: PortionChange) -> Result<Option<u32>, NotAsked> {
    use wellfed_client::calibration::{mean_mg, parse_grams};

    match portion {
        PortionChange::Clear => Ok(None),
        PortionChange::Milligrams { mg } => Ok(Some(mg)),
        PortionChange::Weighed { grams, portions } => {
            let weighed = parse_grams(&grams).map_err(|_| NotAsked::BadWeight)?;
            let mean = mean_mg(weighed, portions).map_err(|_| NotAsked::BadWeight)?;
            Ok(Some(mean))
        }
    }
}

const fn not_edited(error: EditError) -> NotAsked {
    match error {
        EditError::Empty { .. } => NotAsked::EmptySlot,
        EditError::Full => NotAsked::PlansFull,
        EditError::Nothing => NotAsked::NothingToChange,
        EditError::Invalid => NotAsked::BadPlan,
    }
}

fn edit_of(edit: PlanEdit) -> Result<plans::Edit, NotAsked> {
    Ok(match edit {
        PlanEdit::Add { plan } => plans::Edit::Add(plan_of(&plan)),
        PlanEdit::Change {
            slot,
            minute,
            portions,
            days,
            call,
            enabled,
        } => plans::Edit::Change {
            slot,
            at: minute.map(TimeOfDay),
            portions,
            days: days.as_deref().map(weekdays_of),
            call,
            enabled,
        },
        PlanEdit::Enabled { slot, enabled } => plans::Edit::Enabled { slot, enabled },
        PlanEdit::Delete { slot } => plans::Edit::Delete { slot },
    })
}

fn weekdays_of(days: &[Weekday]) -> Weekdays {
    let bits = Weekday::ALL
        .iter()
        .enumerate()
        .filter(|(_, day)| days.contains(day))
        .fold(0u8, |bits, (bit, _)| bits | (1 << bit));
    Weekdays::from_bits(bits)
}

pub(crate) fn days_of(days: Weekdays) -> Vec<Weekday> {
    Weekday::ALL
        .iter()
        .enumerate()
        .filter(|(bit, _)| days.bits() & (1 << bit) != 0)
        .map(|(_, day)| *day)
        .collect()
}

fn plan_of(plan: &PlanView) -> Plan {
    Plan {
        minute: plan.minute,
        portions: plan.portions,
        days: weekdays_of(&plan.days),
        enabled: plan.enabled,
        call: plan.call,
    }
}

fn plans_of(view: &PlansView) -> Plans {
    let mut plans = [None; wellfed_model::PLANS];
    for (slot, plan) in plans.iter_mut().zip(&view.plans) {
        *slot = plan.as_ref().map(plan_of);
    }
    Plans {
        generation: Generation(view.generation),
        writable: view.writable,
        plans,
    }
}

pub(crate) fn plans_view(plans: &Plans) -> PlansView {
    PlansView {
        generation: plans.generation.0,
        writable: plans.writable,
        plans: plans
            .plans
            .iter()
            .map(|plan| {
                plan.map(|plan| PlanView {
                    minute: plan.minute,
                    portions: plan.portions,
                    days: days_of(plan.days),
                    enabled: plan.enabled,
                    call: plan.call,
                })
            })
            .collect(),
    }
}

pub(crate) fn time_info(view: &TimeView) -> TimeInfo {
    let ntp = match view.ntp {
        NtpServer::Off => NtpView::Off,
        NtpServer::Gateway { port } => NtpView::Gateway { port },
        NtpServer::Address { ip, port } => NtpView::Address {
            ip: Ipv4Addr::to_string(&ip),
            port,
        },
    };
    let now_ms = Timestamp::now().as_millisecond();
    TimeInfo {
        generation: view.generation.0,
        writable: view.writable,
        zone: view.zone.clone(),
        ntp,
        clock_ms: view.clock_ms.and_then(|ms| i64::try_from(ms).ok()),
        error_ms: view.error_ms,
        offset_s: view.offset_s,
        syncs: view.syncs,
        last: view.last,
        last_at_ms: view
            .last_age_s
            .map(|age| now_ms.saturating_sub(i64::from(age).saturating_mul(1000))),
        ntp_last: view.ntp_last,
    }
}

pub(crate) const fn refusal_of(refusal: Refusal) -> FeederRefusal {
    match refusal {
        Refusal::Forbidden => FeederRefusal::Forbidden,
        Refusal::Stale { current } => FeederRefusal::Stale { current: current.0 },
        Refusal::Busy => FeederRefusal::Busy,
        Refusal::Unavailable => FeederRefusal::Unavailable,
        Refusal::Invalid => FeederRefusal::Invalid,
        Refusal::NotWritable => FeederRefusal::NotWritable,
        Refusal::Full => FeederRefusal::Full,
        Refusal::NotFound => FeederRefusal::NotFound,
        Refusal::Muted => FeederRefusal::Muted,
        Refusal::Time => FeederRefusal::Time,
        Refusal::Fault => FeederRefusal::Fault,
        Refusal::Update(reason) => FeederRefusal::Update { reason },
        Refusal::Unverified => FeederRefusal::Unverified,
        Refusal::Portions { asked } => FeederRefusal::Portions { asked },
        Refusal::NoClock => FeederRefusal::NoClock,
    }
}

fn call_of(view: CallView) -> CallInfo {
    CallInfo {
        writable: view.writable,
        clip: view.clip,
        ms: view.clip.ms(),
    }
}

fn client_of(client: Client) -> ClientView {
    ClientView {
        fingerprint: wellfed_session::hex::Lower(&client.key.0[..4]).to_string(),
        key: client.key.0.to_vec(),
        label: client.label,
        role: client.role,
        you: client.you,
    }
}

const fn calibration_of(view: CalibrationView) -> CalibrationInfo {
    CalibrationInfo {
        generation: view.generation.0,
        writable: view.writable,
        portion_mg: view.calibration.portion_mg,
        bowl: view.calibration.bowl,
        phase: view.phase,
    }
}

fn invite_of(invite: wellfed_model::Invite, identity: [u8; 32], role: ClientRole) -> InviteView {
    InviteView {
        text: Invite {
            feeder: identity,
            id: invite.id,
            secret: invite.secret.0,
        }
        .text(),
        seconds: invite.seconds,
        role,
    }
}

fn reply_of(answer: Answer) -> FeederReply {
    match answer {
        Answer::Done { generation } => FeederReply::Done {
            generation: generation.map(|generation| generation.0),
        },
        Answer::Settings(settings) => FeederReply::Settings {
            settings: SettingsView {
                generation: settings.generation.0,
                lights: settings.lights,
                sound: settings.sound,
                volume: settings.volume,
                lights_window: settings.lights_window,
                sound_window: settings.sound_window,
                desiccant_days: settings.desiccant_days,
                desiccant_replaced: settings.desiccant_replaced.map(|date| date.to_string()),
                meal_reminder: settings.meal_reminder,
                writable: settings.writable,
            },
        },
        Answer::Time(time) => FeederReply::Time {
            time: time_info(&time),
        },
        Answer::Plans(plans) => FeederReply::Plans {
            plans: plans_view(&plans),
        },
        Answer::Clients(clients) => FeederReply::Clients {
            clients: clients.into_iter().map(client_of).collect(),
        },
        Answer::Calibration(view) => FeederReply::Calibration {
            calibration: calibration_of(view),
        },
        Answer::Call(view) => FeederReply::Call {
            call: call_of(view),
        },
        Answer::Stats(stats) => FeederReply::Stats { stats },
        Answer::Update(view) => FeederReply::Update {
            update: UpdateInfo {
                phase: view.phase,
                bank: view.bank,
                min_version: view.min_version,
                version: view.version,
                image_len: view.image_len,
                written: view.written,
            },
        },
        Answer::Refused(refusal) => FeederReply::Refused {
            refusal: refusal_of(refusal),
        },
        Answer::Invite(_)
        | Answer::Status(_)
        | Answer::Feeding { .. }
        | Answer::FeedTrace(_)
        | Answer::Bank1(_)
        | Answer::StockPlans(_) => FeederReply::Unexpected,
    }
}
