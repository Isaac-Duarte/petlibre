//! `wellfed-feeder-sim`: a simulated feeder on this computer, serving the
//! device API so the server, the app and `wellfed` can run against it.
//!
//! Its flash image lives in `--state`, so its identity, clients, settings
//! and history survive a restart. `--enroll-owner` admits the CLI's key as
//! an owner without BLE setup; after that, invites work as on a feeder.
//! Standard input is the panel: type `help` for the commands.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use clap::Parser;
use wellfed_feeder::store::clients::{Label, Role};
use wellfed_feeder_sim::console::{self, HELP};
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::platform::Random;
use wellfed_feeder_sim::serve::{self, Advertise, Control, Event, Feeder};
use wellfed_formats::hex;
use wellfed_formats::owner::{EnrolledFeeder, OwnerKeyFile};
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_proto::api::PORT;
use wellfed_session::hex::Lower;
use wellfed_session::noise::Keypair;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(version, about = "A simulated Wellfed feeder serving the device API")]
struct Cli {
    /// The directory that keeps the feeder's flash image.
    #[arg(long)]
    state: PathBuf,
    /// Where the device API listens; this computer only, unless a phone or
    /// another machine should reach it (`0.0.0.0:4747`).
    #[arg(long, default_value_t = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)))]
    listen: SocketAddr,
    /// Enroll this `wellfed` key file's key as an owner at every start.
    #[arg(long)]
    enroll_owner: Option<PathBuf>,
    /// Advertise over mDNS with this address, as the feeder does.
    #[arg(long)]
    mdns: Option<Ipv4Addr>,
    /// The update-signing public key (64 hex characters) the simulated
    /// firmware trusts, as a build embeds the bench key's; without one it
    /// refuses every offer.
    #[arg(long)]
    update_key: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.state)
        .with_context(|| format!("create {}", cli.state.display()))?;
    let flash = Flash::file(&cli.state.join("flash.bin"))?;
    let owner = cli.enroll_owner.as_deref().map(owner_key).transpose()?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(cli, flash, owner))
}

/// The key file's public key and label.
fn owner_key(path: &Path) -> Result<([u8; 32], Label)> {
    let file: OwnerKeyFile = wellfed_formats::load(path)
        .with_context(|| format!("read the key file {}", path.display()))?;
    let secret = hex::decode::<32>(&file.secret)
        .with_context(|| format!("{}: the secret is not 32 bytes of hex", path.display()))?;
    let label = Label::new(file.label.as_bytes())
        .with_context(|| format!("{}: the label is not 1-16 bytes", path.display()))?;
    Ok((*Keypair::from_secret(secret).public(), label))
}

/// Record the feeder in the key file, as `wellfed setup` does, so the
/// CLI finds it by identity.
fn remember(path: &Path, identity: &[u8; 32]) -> Result<()> {
    let mut file: OwnerKeyFile = wellfed_formats::load(path)
        .with_context(|| format!("read the key file {}", path.display()))?;
    let identity = hex::encode(identity);
    if file.feeders.iter().any(|f| f.identity == identity) {
        return Ok(());
    }
    file.feeders.push(EnrolledFeeder { identity });
    private::write_json(path, &file).with_context(|| format!("write {}", path.display()))
}

fn system_random() -> Random {
    Box::new(|| private::random(Path::new(SYSTEM_RANDOM)).ok())
}

async fn run(cli: Cli, flash: Rc<Flash>, owner: Option<([u8; 32], Label)>) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("listen on {}", cli.listen))?;
    eprintln!(
        "Device API on {}; state in {}.",
        listener.local_addr()?,
        cli.state.display()
    );
    eprintln!("{HELP}");

    let (control, commands) = mpsc::unbounded_channel();
    tokio::spawn(read_console(control));

    let update_key = match cli.update_key.as_deref() {
        Some(text) => Some(
            wellfed_session::hex::decode_array(text.as_bytes())
                .context("--update-key is not 64 hex characters")?,
        ),
        None => None,
    };
    let feeder = Feeder {
        flash,
        banks: wellfed_feeder_sim::banks::Banks::new(),
        update_key,
        random: Box::new(system_random),
        prepare: Box::new(move |sim| match owner {
            Some((key, label)) => sim.platform().enroll(key, label, Role::Owner),
            None => Ok(()),
        }),
    };
    let advertise = cli.mdns.map(|ip| Advertise { ip });
    let key_file = cli.enroll_owner;
    serve::serve(feeder, listener, advertise, commands, |event| match event {
        Event::Started { identity } => {
            eprintln!("Feeder identity {}.", Lower(&identity));
            if let Some(path) = &key_file
                && let Err(error) = remember(path, &identity)
            {
                eprintln!("{error:#}");
            }
        }
        Event::Sim(record) => eprintln!("{record}"),
        Event::Prepare(error) => eprintln!("--enroll-owner: {error}."),
        Event::Accept(kind) => eprintln!("Accept failed ({kind}); pausing the listener."),
    })
    .await?;
    Ok(())
}

/// Lines from standard input, as the panel's controls. At its end the
/// feeder runs on, so the simulator works with no terminal attached.
async fn read_console(control: mpsc::UnboundedSender<Control>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim() == "help" {
            eprintln!("{HELP}");
            continue;
        }
        match console::parse(&line) {
            Ok(controls) => {
                for c in controls {
                    if control.send(c).is_err() {
                        return;
                    }
                }
            }
            Err(error) => eprintln!("{error}"),
        }
    }
}
