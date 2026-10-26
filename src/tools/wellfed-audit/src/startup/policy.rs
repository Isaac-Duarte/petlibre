//! The manifest's `startup_*_policy` declarations, bound to the image.
//!
//! A declared policy must name a known version, and most are checked both
//! ways against the record formats linked into the image, so a build cannot
//! be audited under another probe's rules nor a probe's records go unchecked.
//! The manifest keeps policies as strings (an unknown one is a refusal here,
//! not a failed load); they become versions once bound.

use wellfed_formats::manifest::BuildManifest;

use super::*;

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
    message: &str,
) -> Result<Option<T>> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    known
        .iter()
        .find(|(name, _)| *name == declared)
        .map(|(_, version)| Some(*version))
        .ok_or_else(|| Error::Refused(message.into()))
}

impl Policies {
    /// Every declared policy, refusing a mismatch with `image` in the
    /// Python auditor's order, so the first refusal reads the same.
    pub(super) fn bind(manifest: &BuildManifest, image: &[u8]) -> Result<Self> {
        let has = |mark: &[u8]| contains(image, mark);

        // No current build declares the probe, so its records mean the
        // image is not the build the manifest describes.
        require(
            !has(SUPERVISION_MARK),
            "Supervision policy/artifact mismatch",
        )?;

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
        "Unknown startup output policy",
    )?;
    require(
        output.is_some() == has(b"PLFW1 outputs pa="),
        "Output policy/artifact mismatch",
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
        "Unknown GPIO clock policy",
    )?
    .is_some();
    require(
        !clock || stopped == Stopped::Required,
        "Clock profile requires stopped-output reporting",
    )?;
    require(
        clock == has(b"PLCLK1 function_before="),
        "GPIO clock policy/artifact mismatch",
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
        "Unknown startup LED policy",
    )?;
    require(
        led.is_some() == has(b"PLLED1 begin"),
        "LED policy/artifact mismatch",
    )?;
    require(
        matches!(led, Some(2..=4)) == has(b" highs="),
        "LED readback format/artifact mismatch",
    )?;
    require(
        (led == Some(3)) == has(b"PLLED1 begin output_only="),
        "LED output-only policy/artifact mismatch",
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
        "Unknown GPIO routing policy",
    )?;
    require(
        routing.is_some() == has(b"PLIO1 stage="),
        "GPIO routing policy/artifact mismatch",
    )?;
    require(
        matches!(routing, Some(2 | 3)) == has(b" share_pad="),
        "GPIO routing format/artifact mismatch",
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
        "Unknown shared-pad policy",
    )?;
    // Review 06 and earlier print PLPWR1 after KM4 boot without declaring this policy.
    require(
        power.is_none() || has(b"PLPWR1 before_func="),
        "Shared-pad policy/artifact mismatch",
    )?;
    // Checked both ways: a before-KM4 v3 image cannot be audited without its policy.
    require(
        (power == Some(3)) == (has(b"PLPWR1 before_func=") && has(POWER_V3_FORMAT)),
        "Shared-pad v3 policy/artifact mismatch",
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
        "Unknown Wi-Fi policy",
    )?;
    require(
        (wifi == Some(Wifi::Scan)) == has(WIFI_MARK),
        "Wi-Fi policy/artifact mismatch",
    )?;
    require(
        (wifi == Some(Wifi::Join)) == has(WIFI_JOIN_MARK),
        "Wi-Fi join policy/artifact mismatch",
    )?;
    require(
        (wifi == Some(Wifi::Keep)) == has(WIFI_KEEP_MARK),
        "Wi-Fi keep policy/artifact mismatch",
    )?;
    require(
        wifi.is_none() || led,
        "Wi-Fi probe requires the LED probe policy",
    )?;

    let transport = version(
        manifest.startup_transport_policy.as_deref(),
        &[("fw08-transport-v1", ())],
        "Unknown transport policy",
    )?
    .is_some();
    require(
        transport == has(TRANSPORT_MARK),
        "Transport policy/artifact mismatch",
    )?;
    require(
        !transport || wifi == Some(Wifi::Keep),
        "Transport requires the keep policy",
    )?;

    let optics = version(
        manifest.startup_optics_policy.as_deref(),
        &[("fw13-optics-v1", 1), ("fw13-optics-v2", 2)],
        "Unknown optics policy",
    )?;
    require(
        optics.is_some() == has(OPTICS_MARK),
        "Optics policy/artifact mismatch",
    )?;
    require(
        (optics == Some(2)) == has(&[OPTICS_MARK, b"1", OPTICS_PLAN2_MARK].concat()),
        "Optics plan/artifact mismatch",
    )?;
    require(
        optics.is_none() || led,
        "Optics probe requires the LED probe policy",
    )?;

    let motion = version(
        manifest.startup_motion_policy.as_deref(),
        &[("fw16-pulse-v1", ())],
        "Unknown motion policy",
    )?
    .is_some();
    require(
        motion == has(MOTION_MARK),
        "Motion policy/artifact mismatch",
    )?;
    require(
        !(motion && optics.is_some()),
        "Motion and optics probes are exclusive",
    )?;
    require(!motion || led, "Motion probe requires the LED probe policy")?;

    let trial = version(
        manifest.startup_trial_policy.as_deref(),
        &[("fw11-reset-v1", ())],
        "Unknown trial policy",
    )?
    .is_some();
    require(
        trial == has(TRIAL_RESET_MARK),
        "Trial policy/artifact mismatch",
    )?;
    require(
        !(trial && (optics.is_some() || motion)),
        "The trial test runs alone",
    )?;

    let adc = version(
        manifest.startup_adc_policy.as_deref(),
        &[("fw14-adc-v1", 1), ("fw14-adc-v2", 2)],
        "Unknown ADC policy",
    )?;
    require(
        adc.is_some() == has(ADC_MARK),
        "ADC policy/artifact mismatch",
    )?;
    require(
        (adc == Some(2)) == has(ADC_SUPPLY_MARK),
        "ADC supply policy/artifact mismatch",
    )?;
    require(
        adc.is_none() || led,
        "ADC probe requires the LED probe policy",
    )
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
        "Unknown flash handshake policy",
    )?;
    require(
        flash.is_some() == has(FLASH_MARK),
        "Flash handshake policy/artifact mismatch",
    )?;

    // Checked both ways: a park image cannot be audited as a hello-only build, nor the reverse.
    for (policy, mark, message) in [
        (
            FlashPolicy::Park,
            PARK_MARK,
            "Flash park policy/artifact mismatch",
        ),
        (
            FlashPolicy::Read,
            READ_MARK,
            "Flash read policy/artifact mismatch",
        ),
        (
            FlashPolicy::Write,
            WRITE_MARK,
            "Flash write policy/artifact mismatch",
        ),
        (
            FlashPolicy::Journal,
            JOURNAL_MARK,
            "Flash journal policy/artifact mismatch",
        ),
        (
            FlashPolicy::Settings,
            SETTINGS_MARK,
            "Flash settings policy/artifact mismatch",
        ),
    ] {
        require((flash == Some(policy)) == has(mark), message)?;
    }
    Ok(flash)
}
