//! Enrolling an owner's update-signing key, confirmed by a Dispense press
//! at the feeder. What is sent and what the answers mean is
//! `wellfed_client::update_key`'s; this is the IO, shared by the install's
//! key step and `wellfed update-key enroll`.

use std::time::Duration;

use wellfed_client::update_key::{Enroll, EnrollError, Enrolled, Next};
use wellfed_model::{Answer, Client, Op};

use crate::Error;
use crate::event::Event;
use crate::platform::{Announcement, PlatformError, Prompter, Report};
use crate::session::Session;

/// How long after the enrollment goes out the press is asked for. The
/// feeder takes a Dispense press as the confirmation only once the request
/// waits for it, and nothing tells the run when that is: this is a timing
/// assumption (a LAN request arrives within milliseconds; the window is
/// 60 s), not a signal. If it fails, the press comes first: on a consumer
/// image's panel, locked since its boot, that press does nothing, the
/// window closes `Unconfirmed`, and the run stops with no second request.
pub const PRESS_AFTER: Duration = Duration::from_secs(1);

/// Run one enrollment of the key `public` (its id `id`) over `session` to
/// its end, `prompter`
/// announcing the press: the outer error is a failure before anything was
/// asked (a prompt that could not be settled), the inner one how the
/// feeder ended it. The enrollment goes at most once.
pub async fn enroll(
    session: &mut impl Session,
    public: [u8; 32],
    id: [u8; 8],
    label: &str,
    prompter: &mut impl Prompter,
    report: &impl Report,
) -> Result<Result<Enrolled, EnrollError>, Error> {
    let mut enroll = match Enroll::new(public, label, session.role(), session.capabilities()) {
        Ok(enroll) => enroll,
        Err(error) => return Ok(Err(error)),
    };

    loop {
        let answered = match enroll.poll() {
            Next::Send(op) => session.ask(&op).await,
            Next::Press(op) => {
                // A prompt answered earlier is read before the request goes,
                // so no wait for it spends the feeder's window.
                prompter.settle().await.map_err(Error::Platform)?;
                pressed(session, &op, id, prompter, report).await
            }
            Next::Done(enrolled) => return Ok(Ok(enrolled)),
            Next::Failed(error) => return Ok(Err(error)),
        };
        match answered {
            Ok(answer) => enroll.answer(&answer),
            Err(error) => {
                report.event(Event::EnrollUnanswered {
                    error: crate::text(&*error),
                });
                enroll.unanswered();
            }
        }
    }
}

/// Send the enrollment `op` and, once it has had [`PRESS_AFTER`] to reach
/// the feeder unanswered, announce the Dispense press; its answer. A
/// refusal that comes back at once (busy, say) asks for no press. The
/// announcement never stops the request: one already sent is judged by its
/// answer alone, so a failed announcement is only reported.
async fn pressed(
    session: &mut impl Session,
    op: &Op,
    key: [u8; 8],
    prompter: &mut impl Prompter,
    report: &impl Report,
) -> Result<Answer, PlatformError> {
    let asked = session.ask(op);
    tokio::pin!(asked);
    tokio::select! {
        answer = &mut asked => answer,
        () = tokio::time::sleep(PRESS_AFTER) => {
            if let Err(error) = prompter.announce(Announcement::PressDispense { key }).await {
                report.event(Event::PressNotAnnounced { error: error.to_string() });
            }
            asked.await
        }
    }
}

/// The feeder's enrolled clients.
pub async fn clients(session: &mut impl Session) -> Result<Vec<Client>, Error> {
    match session
        .ask(&Op::ListClients)
        .await
        .map_err(Error::Platform)?
    {
        Answer::Clients(clients) => Ok(clients),
        Answer::Refused(refusal) => Err(Error::ClientsRefused(refusal)),
        other => Err(Error::Answered {
            request: "client list",
            answer: Box::new(other),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wellfed_feeder_sim::console;
    use wellfed_feeder_sim::harness::Threaded;
    use wellfed_formats::credentials::KeyScope;
    use wellfed_model::ClientRole;
    use wellfed_session::noise::Keypair;
    use wellfed_signing::Key;

    use super::*;
    use crate::tests::Scripted;

    const OWNER: [u8; 32] = [0x21; 32];

    /// The bench key's file enrolls as an owner's does, with a press, and a
    /// second enrollment of a listed key sends nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_key_enrolls_with_a_press_and_then_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ota-key.json");
        wellfed_signing::keygen(&file, KeyScope::Bench).unwrap();
        let key = Key::load(&file).unwrap();

        let feeder = Threaded::start(*Keypair::from_secret(OWNER).public());
        let keypair = Keypair::from_secret(OWNER);
        let mut session =
            wellfed_connect::Connection::open(&keypair, feeder.identity, feeder.address, [9; 32])
                .await
                .unwrap();
        for control in console::parse("hold lock").unwrap() {
            feeder.control(control);
        }

        let hands = async {
            // After the press is asked for, as a person or the bench would.
            tokio::time::sleep(PRESS_AFTER + Duration::from_millis(500)).await;
            for control in console::parse("press dispense").unwrap() {
                feeder.control(control);
            }
        };
        let mut prompter = Scripted::default();
        let report = |_| {};
        let (enrolled, ()) = tokio::join!(
            enroll(
                &mut session,
                key.public(),
                key.id(),
                "bench",
                &mut prompter,
                &report
            ),
            hands
        );
        assert_eq!(enrolled.unwrap(), Ok(Enrolled::Now));
        // The press was announced once the request was out.
        assert_eq!(
            prompter.announced,
            [Announcement::PressDispense { key: key.id() }]
        );
        let listed = clients(&mut session).await.unwrap();
        let entry = listed.iter().find(|c| c.key.0 == key.public()).unwrap();
        assert_eq!(entry.role, ClientRole::UpdateSigner);

        // Again: the list says it is enrolled, so nothing else goes out
        // (an enrollment now would wait its 60 s window for a press).
        let again = tokio::time::timeout(
            Duration::from_secs(5),
            enroll(
                &mut session,
                key.public(),
                key.id(),
                "bench",
                &mut prompter,
                &report,
            ),
        )
        .await;
        assert!(matches!(again, Ok(Ok(Ok(Enrolled::Already)))), "{again:?}");
    }
}
