//! The manifest's `startup_*_policy` declarations, bound to the image.
//!
//! A declared policy must name a known version, and most are checked both
//! ways against the record formats linked into the image, so a build cannot
//! be audited under another probe's rules nor a probe's records go unchecked.
//! The manifest keeps policies as strings (an unknown one is a refusal here,
//! not a failed load); they become versions once bound.

use wellfed_formats::manifest::BuildManifest;

use super::*;

/// A declared policy refused before any record is classified: unknown, at
/// odds with the record formats in the image, or missing a probe it needs.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// The manifest names a version this audit does not know.
    #[error("Unknown {policy}")]
    Unknown { policy: Policy, declared: String },
    /// The declaration and the image's record formats disagree.
    #[error("{declaration}/artifact mismatch")]
    Mismatch { declaration: Declaration },
    /// The probe's records follow the LED probe's, so it needs that policy.
    #[error("{probe} probe requires the LED probe policy")]
    RequiresLed { probe: Probe },
    #[error("Clock profile requires stopped-output reporting")]
    ClockWithoutStopped,
    #[error("Transport requires the keep policy")]
    TransportWithoutKeep,
    #[error("Motion and optics probes are exclusive")]
    MotionWithOptics,
    #[error("The trial test runs alone")]
    TrialNotAlone,
}

crate::outcomes! {
    /// A manifest's `startup_*_policy` field, as its unknown-version
    /// refusal names it.
    pub enum Policy {
        Output => "startup output policy",
        GpioClock => "GPIO clock policy",
        Led => "startup LED policy",
        GpioRouting => "GPIO routing policy",
        SharedPad => "shared-pad policy",
        Wifi => "Wi-Fi policy",
        Transport => "transport policy",
        Optics => "optics policy",
        Motion => "motion policy",
        Trial => "trial policy",
        Adc => "ADC policy",
        FlashHandshake => "flash handshake policy",
    }
}

crate::outcomes! {
    /// What a policy declares about the image, checked against its record
    /// formats: the policy itself, or a format or plan within it.
    pub enum Declaration {
        Supervision => "Supervision policy",
        Output => "Output policy",
        GpioClock => "GPIO clock policy",
        Led => "LED policy",
        LedReadback => "LED readback format",
        LedOutputOnly => "LED output-only policy",
        GpioRouting => "GPIO routing policy",
        GpioRoutingFormat => "GPIO routing format",
        SharedPad => "Shared-pad policy",
        SharedPadV3 => "Shared-pad v3 policy",
        Wifi => "Wi-Fi policy",
        WifiJoin => "Wi-Fi join policy",
        WifiKeep => "Wi-Fi keep policy",
        Transport => "Transport policy",
        Optics => "Optics policy",
        OpticsPlan => "Optics plan",
        Motion => "Motion policy",
        Trial => "Trial policy",
        Adc => "ADC policy",
        AdcSupply => "ADC supply policy",
        FlashHandshake => "Flash handshake policy",
        FlashPark => "Flash park policy",
        FlashRead => "Flash read policy",
        FlashWrite => "Flash write policy",
        FlashJournal => "Flash journal policy",
        FlashSettings => "Flash settings policy",
    }
}

crate::outcomes! {
    /// A probe that runs after the LED probe.
    pub enum Probe {
        Wifi => "Wi-Fi",
        Optics => "Optics",
        Motion => "Motion",
        Adc => "ADC",
    }
}

/// The probes a startup is classified under, once bound to the image.
pub(super) struct Policies {
    pub(super) stopped: Stopped,
    pub(super) clock: bool,
    /// Each probe's record version (see its `classify_*`).
    pub(super) led: Option<u8>,
    pub(super) routing: Option<u8>,
    pub(super) power: Option<u8>,
    pub(super) flash: Option<FlashPolicy>,
}

/// A declared policy's version: absent (None) or one of `known`.
fn version<T: Copy>(
    declared: Option<&str>,
    known: &[(&str, T)],
    policy: Policy,
) -> Result<Option<T>> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    let (_, version) = known
        .iter()
        .find(|(name, _)| *name == declared)
        .ok_or_else(|| PolicyError::Unknown {
            policy,
            declared: declared.into(),
        })?;
    Ok(Some(*version))
}

/// Refuse unless the declaration and the image agree.
fn agree(agrees: bool, declaration: Declaration) -> Result<()> {
    require(agrees, || PolicyError::Mismatch { declaration }.into())
}

/// Refuse a probe declared without the LED probe it follows.
fn after_led(declared: bool, led: bool, probe: Probe) -> Result<()> {
    require(!declared || led, || {
        PolicyError::RequiresLed { probe }.into()
    })
}

impl Policies {
    /// Every declared policy, refusing a mismatch with `image` in the
    /// Python auditor's order, so the first refusal reads the same.
    pub(super) fn bind(manifest: &BuildManifest, image: &[u8]) -> Result<Self> {
        let has = |mark: &[u8]| contains(image, mark);

        // No current build declares the probe, so its records mean the
        // image is not the build the manifest describes.
        agree(!has(SUPERVISION_MARK), Declaration::Supervision)?;

        let stopped = bind_outputs(manifest, &has)?;
        let clock = bind_clock(manifest, &has, stopped)?;
        let led = bind_led(manifest, &has)?;
        let routing = bind_routing(manifest, &has)?;
        let power = bind_power(manifest, &has)?;
        bind_unclassified(manifest, &has, led.is_some())?;
        let flash = bind_flash(manifest, &has)?;

        Ok(Self {
            stopped,
            clock,
            led,
            routing,
            power,
            flash,
        })
    }
}

fn bind_outputs(manifest: &BuildManifest, has: &dyn Fn(&[u8]) -> bool) -> Result<Stopped> {
    let output = version(
        manifest.startup_output_policy.as_deref(),
        &[("stop-only", ())],
        Policy::Output,
    )?;
    agree(
        output.is_some() == has(b"PLFW1 outputs pa="),
        Declaration::Output,
    )?;

    Ok(if output.is_some() {
        Stopped::Required
    } else {
        Stopped::Optional
    })
}

fn bind_clock(
    manifest: &BuildManifest,
    has: &dyn Fn(&[u8]) -> bool,
    stopped: Stopped,
) -> Result<bool> {
    let clock = version(
        manifest.startup_gpio_clock_policy.as_deref(),
        &[("explicit-enable-v1", ())],
        Policy::GpioClock,
    )?
    .is_some();
    require(!clock || stopped == Stopped::Required, || {
        PolicyError::ClockWithoutStopped
    })?;
    agree(
        clock == has(b"PLCLK1 function_before="),
        Declaration::GpioClock,
    )?;
    Ok(clock)
}

fn bind_led(manifest: &BuildManifest, has: &dyn Fn(&[u8]) -> bool) -> Result<Option<u8>> {
    let led = version(
        manifest.startup_led_policy.as_deref(),
        &[
            ("bounded-probe-v1", 1),
            ("bounded-probe-v2", 2),
            ("bounded-probe-v3", 3),
            ("low-only-v1", 4),
        ],
        Policy::Led,
    )?;
    agree(led.is_some() == has(b"PLLED1 begin"), Declaration::Led)?;
    agree(
        matches!(led, Some(2..=4)) == has(b" highs="),
        Declaration::LedReadback,
    )?;
    agree(
        (led == Some(3)) == has(b"PLLED1 begin output_only="),
        Declaration::LedOutputOnly,
    )?;
    Ok(led)
}

fn bind_routing(manifest: &BuildManifest, has: &dyn Fn(&[u8]) -> bool) -> Result<Option<u8>> {
    let routing = version(
        manifest.startup_gpio_routing_policy.as_deref(),
        &[
            ("read-only-v1", 1),
            ("read-only-v2", 2),
            ("read-only-v3", 3),
        ],
        Policy::GpioRouting,
    )?;
    agree(
        routing.is_some() == has(b"PLIO1 stage="),
        Declaration::GpioRouting,
    )?;
    agree(
        matches!(routing, Some(2 | 3)) == has(b" share_pad="),
        Declaration::GpioRoutingFormat,
    )?;
    Ok(routing)
}

fn bind_power(manifest: &BuildManifest, has: &dyn Fn(&[u8]) -> bool) -> Result<Option<u8>> {
    let power = version(
        manifest.startup_shared_pad_policy.as_deref(),
        &[
            ("before-km4-v1", 1),
            ("before-km4-v2", 2),
            ("before-km4-v3", 3),
        ],
        Policy::SharedPad,
    )?;
    // Review 06 and earlier print PLPWR1 after KM4 boot without declaring this policy.
    agree(
        power.is_none() || has(b"PLPWR1 before_func="),
        Declaration::SharedPad,
    )?;
    // Checked both ways: a before-KM4 v3 image cannot be audited without its policy.
    agree(
        (power == Some(3)) == (has(b"PLPWR1 before_func=") && has(POWER_V3_FORMAT)),
        Declaration::SharedPadV3,
    )?;
    Ok(power)
}

/// The probes this audit only binds to the image, without classifying
/// their records. Each needs the LED probe, which they follow.
fn bind_unclassified(
    manifest: &BuildManifest,
    has: &dyn Fn(&[u8]) -> bool,
    led: bool,
) -> Result<()> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Wifi {
        Scan,
        Join,
        /// Join and keep the link. Images before the gateway watch named it
        /// for their internet DNS check; the record format is the same.
        Keep,
    }
    let wifi = version(
        manifest.startup_wifi_policy.as_deref(),
        &[
            ("fw07-scan-v1", Wifi::Scan),
            ("fw07-join-v1", Wifi::Join),
            // Drop once no frozen review in use declares it: every image
            // built since the gateway watch declares wifi-gateway-v1.
            ("fw07-dns-v1", Wifi::Keep),
            ("wifi-gateway-v1", Wifi::Keep),
        ],
        Policy::Wifi,
    )?;
    agree(
        (wifi == Some(Wifi::Scan)) == has(WIFI_MARK),
        Declaration::Wifi,
    )?;
    agree(
        (wifi == Some(Wifi::Join)) == has(WIFI_JOIN_MARK),
        Declaration::WifiJoin,
    )?;
    agree(
        (wifi == Some(Wifi::Keep)) == has(WIFI_KEEP_MARK),
        Declaration::WifiKeep,
    )?;
    after_led(wifi.is_some(), led, Probe::Wifi)?;

    let transport = version(
        manifest.startup_transport_policy.as_deref(),
        &[("fw08-transport-v1", ())],
        Policy::Transport,
    )?
    .is_some();
    agree(transport == has(TRANSPORT_MARK), Declaration::Transport)?;
    require(!transport || wifi == Some(Wifi::Keep), || {
        PolicyError::TransportWithoutKeep
    })?;

    let optics = version(
        manifest.startup_optics_policy.as_deref(),
        &[("fw13-optics-v1", 1), ("fw13-optics-v2", 2)],
        Policy::Optics,
    )?;
    agree(optics.is_some() == has(OPTICS_MARK), Declaration::Optics)?;
    agree(
        (optics == Some(2)) == has(&[OPTICS_MARK, b"1", OPTICS_PLAN2_MARK].concat()),
        Declaration::OpticsPlan,
    )?;
    after_led(optics.is_some(), led, Probe::Optics)?;

    let motion = version(
        manifest.startup_motion_policy.as_deref(),
        &[("fw16-pulse-v1", ())],
        Policy::Motion,
    )?
    .is_some();
    agree(motion == has(MOTION_MARK), Declaration::Motion)?;
    require(!(motion && optics.is_some()), || {
        PolicyError::MotionWithOptics
    })?;
    after_led(motion, led, Probe::Motion)?;

    let trial = version(
        manifest.startup_trial_policy.as_deref(),
        &[("fw11-reset-v1", ())],
        Policy::Trial,
    )?
    .is_some();
    agree(trial == has(TRIAL_RESET_MARK), Declaration::Trial)?;
    require(!(trial && (optics.is_some() || motion)), || {
        PolicyError::TrialNotAlone
    })?;

    let adc = version(
        manifest.startup_adc_policy.as_deref(),
        &[("fw14-adc-v1", 1), ("fw14-adc-v2", 2)],
        Policy::Adc,
    )?;
    agree(adc.is_some() == has(ADC_MARK), Declaration::Adc)?;
    agree(
        (adc == Some(2)) == has(ADC_SUPPLY_MARK),
        Declaration::AdcSupply,
    )?;
    after_led(adc.is_some(), led, Probe::Adc)
}

fn bind_flash(
    manifest: &BuildManifest,
    has: &dyn Fn(&[u8]) -> bool,
) -> Result<Option<FlashPolicy>> {
    let flash = version(
        manifest.startup_flash_handshake_policy.as_deref(),
        &[
            ("fw06-hello-v1", FlashPolicy::Hello),
            ("fw06-park-v1", FlashPolicy::Park),
            ("fw06-read-v1", FlashPolicy::Read),
            ("fw06-write-v1", FlashPolicy::Write),
            ("fw15-journal-v1", FlashPolicy::Journal),
            ("fw15-settings-v1", FlashPolicy::Settings),
            ("fw15-settings-v2", FlashPolicy::Settings),
        ],
        Policy::FlashHandshake,
    )?;
    agree(
        flash.is_some() == has(FLASH_MARK),
        Declaration::FlashHandshake,
    )?;

    // Checked both ways: a park image cannot be audited as a hello-only build, nor the reverse.
    for (policy, mark, declaration) in [
        (FlashPolicy::Park, PARK_MARK, Declaration::FlashPark),
        (FlashPolicy::Read, READ_MARK, Declaration::FlashRead),
        (FlashPolicy::Write, WRITE_MARK, Declaration::FlashWrite),
        (
            FlashPolicy::Journal,
            JOURNAL_MARK,
            Declaration::FlashJournal,
        ),
        (
            FlashPolicy::Settings,
            SETTINGS_MARK,
            Declaration::FlashSettings,
        ),
    ] {
        agree((flash == Some(policy)) == has(mark), declaration)?;
    }
    Ok(flash)
}
