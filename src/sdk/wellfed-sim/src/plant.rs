//! The feed mechanism as the bench measured it (physical, October 25, the
//! first feeds through the owner; `docs/firmware/motor-pulse.md`): a
//! three-compartment rotor whose feedback input, PA18, rests HIGH and reads
//! LOW for about 1.2 s at each compartment's mark. A portion is one
//! compartment, credited on the mark's rise, about 9.07 s of drive empty and
//! 9.8 s with food. Feed 9's edges, from the drive's start just past a
//! rise, were `7840L,9037H,16970L,18156H,26047L,27221H`.
//!
//! The rotor turns only while the motor drives, so the plant is a function
//! of its position, the drive time it has accumulated (forward on `Drive0`,
//! back on `Drive1`): the same position always reads the same level, across
//! stops and reboots, and the harness carries it from one worker to the next
//! as the rotor keeps its place through a power cut. Each mark's period and
//! LOW width vary by up to their jitter, and each of its two transitions may
//! be missed by the capture (the level changes, no interrupt comes) or
//! bounce (the level flips back and again, each flip interrupting), all
//! drawn from the seed and the mark, so a scenario replays exactly. Position
//! 0 is just past a rise, where the owner stops.
//!
//! The motor current follows the same captures: a start-up surge over the
//! 300 limit that a poll saw 17 ms into a drive (scaled 333) and that no two
//! consecutive polls saw in the feeds after it, then about 60 scaled while
//! driving empty, and the stopped reading.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// The mechanism's parameters; every field defaults to the captures'.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlantConfig {
    /// Draws every mark's jitter, misses and bounces.
    pub seed: u64,
    /// Drive time from one mark's rise to the next's: 9.04-9.12 s empty in
    /// feed 9 (about 9.8 s with food).
    pub period_ms: u32,
    pub period_jitter_ms: u32,
    /// How long PA18 reads LOW at a mark: 1.17-1.2 s empty (about 1.38 s
    /// with food).
    pub low_ms: u32,
    pub low_jitter_ms: u32,
    /// Transitions in a thousand the capture misses.
    pub missed_permille: u16,
    /// Transitions in a thousand that bounce: the level flips back after
    /// 1 ms and again after `bounce_ms` more.
    pub double_permille: u16,
    pub bounce_ms: u32,
    /// How long a drive's start-up surge reads, from the drive's start: the
    /// 17 ms reading, and no more than one 20 ms poll period.
    pub surge_ms: u32,
    /// The current channel's raw readings: the surge, driving (a floor and
    /// a ripple above it, skewed toward the floor), and stopped.
    pub surge_adc: u16,
    pub drive_adc: u16,
    pub ripple_adc: u16,
    pub idle_adc: u16,
}

impl Default for PlantConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            // Feed 9's rises, 9037, 18156 and 27221 ms: 9074 ms apart on
            // average, the widest 45 ms off it.
            period_ms: 9074,
            period_jitter_ms: 50,
            // Its marks: 1197, 1186 and 1174 ms.
            low_ms: 1186,
            low_jitter_ms: 15,
            missed_permille: 0,
            double_permille: 0,
            bounce_ms: 2,
            surge_ms: 20,
            // Raw readings for the default calibration (offset 2480, gain
            // 12050): 650 scales to 334, 16 to 0. Driving empty, feeds read
            // a mean of 59-64 and a maximum of 164-168: a floor of 280 (26)
            // with a ripple up to 450 (167) averages 322 (61).
            surge_adc: 650,
            drive_adc: 280,
            ripple_adc: 170,
            idle_adc: 16,
        }
    }
}

impl PlantConfig {
    /// Whether each mark's transitions come in order: a bounce ends inside
    /// its level, and a mark's LOW inside its period.
    pub fn valid(&self) -> bool {
        let bounce = 2 + u64::from(self.bounce_ms);
        let low_min = u64::from(self.low_ms).saturating_sub(u64::from(self.low_jitter_ms));
        let low_max = u64::from(self.low_ms) + u64::from(self.low_jitter_ms);
        let period_min = u64::from(self.period_ms).saturating_sub(u64::from(self.period_jitter_ms));
        self.low_jitter_ms < self.low_ms
            && self.period_jitter_ms < self.period_ms
            && low_min > bounce
            && period_min > low_max + bounce
            && self.missed_permille <= 1000
            && self.double_permille <= 1000
            && self.bounce_ms <= 100
    }
}

/// One change of PA18's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flip {
    pub high: bool,
    /// Whether the capture's interrupt sees it.
    pub captured: bool,
}

/// Which way the motor turns the rotor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Forward,
    Back,
}

/// One laid-out mark: its flips, in position order.
type Mark = Vec<(i64, Flip)>;

/// The rotor, at a position.
#[derive(Clone, Debug)]
pub struct Plant {
    config: PlantConfig,
    position_ms: i64,
    level: bool,
    /// The marks laid out so far, in order, and the index of the first.
    marks: VecDeque<Mark>,
    first: i64,
    /// The rises around them: before the first mark's, and the last's.
    rise_before: i64,
    rise_last: i64,
}

/// Separates the ripple's draws from the marks'.
const RIPPLE: u64 = 0x6375_7272_656e_7421;

/// SplitMix64: a fixed, portable draw per seed and index.
fn draw(seed: u64, index: u64) -> u64 {
    let mut z = seed ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `base` moved by up to `jitter` either way.
fn jittered(base: u32, jitter: u32, draw: u64) -> i64 {
    let span = 2 * u64::from(jitter) + 1;
    let offset = i64::try_from(draw % span).expect("below 2^33");
    i64::from(base) + offset - i64::from(jitter)
}

fn chance(permille: u16, draw: u64) -> bool {
    draw % 1000 < u64::from(permille)
}

impl Plant {
    /// The rotor `position_ms` of drive past position 0.
    pub fn new(config: PlantConfig, position_ms: i64) -> Self {
        let mut plant = Self {
            config,
            position_ms,
            level: true,
            marks: VecDeque::new(),
            first: 0,
            rise_before: 0,
            rise_last: 0,
        };
        plant.cover(position_ms);
        // The level after the last flip at or before the position, or
        // before the first flip after it.
        let flips = plant.marks.iter().flatten();
        plant.level = match flips.clone().rev().find(|(at, _)| *at <= position_ms) {
            Some((_, flip)) => flip.high,
            None => flips.clone().next().is_none_or(|(_, flip)| !flip.high),
        };
        plant
    }

    pub fn config(&self) -> &PlantConfig {
        &self.config
    }

    pub fn position_ms(&self) -> i64 {
        self.position_ms
    }

    /// The current channel while driving here: the floor, plus a ripple
    /// drawn per position, cubed toward the floor so its mean stays a
    /// quarter of the way up and its peaks are rare.
    pub fn drive_reading(&self) -> u16 {
        let c = self.config;
        let x = draw(c.seed ^ RIPPLE, self.position_ms.cast_unsigned()) % 1001;
        let up = u64::from(c.ripple_adc) * x * x * x / 1_000_000_000;
        let up = u16::try_from(up).expect("at most the ripple");
        c.drive_adc.saturating_add(up)
    }

    /// PA18's level here.
    pub fn level(&self) -> bool {
        self.level
    }

    /// One more millisecond of drive: the flips it passes, in order.
    pub fn drive(&mut self, turn: Turn) -> Vec<Flip> {
        let (from, to) = match turn {
            Turn::Forward => (self.position_ms, self.position_ms + 1),
            Turn::Back => (self.position_ms, self.position_ms - 1),
        };
        self.cover(to);
        self.position_ms = to;

        // A mark's flips are in order and the marks' do not interleave, so
        // the flips at one position are all in the first mark that reaches it.
        let at = match turn {
            Turn::Forward => to,
            Turn::Back => from,
        };
        let index = self
            .marks
            .partition_point(|mark| mark.last().is_none_or(|(last, _)| *last < at));
        let here = self
            .marks
            .get(index)
            .into_iter()
            .flatten()
            .filter(|(position, _)| *position == at)
            .map(|(_, flip)| *flip);
        let flips: Vec<Flip> = match turn {
            Turn::Forward => here.collect(),
            // Undone last first, each to the level before it.
            Turn::Back => {
                let mut undone: Vec<Flip> = here
                    .map(|flip| Flip {
                        high: !flip.high,
                        ..flip
                    })
                    .collect();
                undone.reverse();
                undone
            }
        };
        if let Some(last) = flips.last() {
            self.level = last.high;
        }
        flips
    }

    /// Lay out marks until every flip within a period of `position` is:
    /// mark `k` takes the drive between the rises of marks `k - 1` and `k`,
    /// and mark 0 starts at position 0's rise.
    fn cover(&mut self, position: i64) {
        let margin = i64::from(self.config.period_ms);
        while self.rise_last <= position + margin {
            let index = self.first + i64::try_from(self.marks.len()).expect("few marks");
            let (mark, rise) = self.mark(index, self.rise_last);
            self.marks.push_back(mark);
            self.rise_last = rise;
        }
        while self.rise_before >= position - margin {
            let index = self.first - 1;
            let before = self.rise_before - self.period(index);
            let (mark, _) = self.mark(index, before);
            self.marks.push_front(mark);
            self.first = index;
            self.rise_before = before;
        }
    }

    fn draws(&self, index: i64) -> impl Fn(u64) -> u64 {
        let seed = self.config.seed;
        let base = index.cast_unsigned().wrapping_mul(8);
        move |field| draw(seed, base.wrapping_add(field))
    }

    fn period(&self, index: i64) -> i64 {
        let c = self.config;
        jittered(c.period_ms, c.period_jitter_ms, self.draws(index)(0))
    }

    /// Mark `index`'s flips after the rise at `rise_before`, and its rise.
    fn mark(&self, index: i64, rise_before: i64) -> (Mark, i64) {
        let c = self.config;
        let d = self.draws(index);
        let low = jittered(c.low_ms, c.low_jitter_ms, d(1));
        let rise = rise_before + self.period(index);
        let fall = rise - low;
        let bounce = i64::from(c.bounce_ms);

        let mut flips = Vec::new();
        for (at, high, field) in [(fall, false, 2), (rise, true, 4)] {
            let captured = !chance(c.missed_permille, d(field));
            flips.push((at, Flip { high, captured }));
            if chance(c.double_permille, d(field + 1)) {
                let back = Flip {
                    high: !high,
                    captured,
                };
                flips.push((at + 1, back));
                flips.push((at + 1 + bounce, Flip { high, captured }));
            }
        }
        (flips, rise)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flip of `plant` over `ms` of drive, with its position.
    fn run(plant: &mut Plant, ms: u64, turn: Turn) -> Vec<(i64, Flip)> {
        (0..ms)
            .flat_map(|_| {
                let flips = plant.drive(turn);
                let at = plant.position_ms();
                flips.into_iter().map(move |flip| (at, flip))
            })
            .collect()
    }

    #[test]
    fn the_default_plant_matches_the_three_portion_capture() {
        let config = PlantConfig::default();
        assert!(config.valid());
        let mut plant = Plant::new(config, 0);
        assert!(plant.level(), "PA18 rests HIGH past a rise");

        let flips = run(&mut plant, 27_300, Turn::Forward);
        let bench = [7840, 9037, 16970, 18156, 26047, 27221];
        assert_eq!(flips.len(), bench.len());
        for (index, ((at, flip), want)) in flips.iter().zip(bench).enumerate() {
            assert_eq!(flip.high, index % 2 == 1);
            assert!(flip.captured);
            // Within the jitter of the bench's own edges.
            assert!(at.abs_diff(want) <= 100, "{at} for {want}");
        }

        // Each mark reads LOW for its width, and each rise is a portion.
        for pair in flips.chunks(2) {
            let low = pair[1].0 - pair[0].0;
            assert!((1171..=1201).contains(&low), "{low}");
        }
        let rises: Vec<i64> = flips
            .iter()
            .filter(|(_, f)| f.high)
            .map(|(at, _)| *at)
            .collect();
        for gap in rises.windows(2) {
            assert!((9024..=9124).contains(&(gap[1] - gap[0])));
        }
    }

    #[test]
    fn the_rotor_keeps_its_place_across_stops_and_restarts() {
        let config = PlantConfig {
            seed: 7,
            double_permille: 300,
            missed_permille: 200,
            ..PlantConfig::default()
        };
        let mut whole = Plant::new(config, 0);
        let all = run(&mut whole, 60_000, Turn::Forward);

        // Driven in pieces, or rebuilt at a position, it flips the same way.
        let mut pieces = Vec::new();
        let mut position = 0;
        for piece in [1, 4_000, 3_877, 1, 20_000, 32_121] {
            let mut plant = Plant::new(config, position);
            pieces.extend(run(&mut plant, piece, Turn::Forward));
            position = plant.position_ms();
        }
        assert_eq!(position, 60_000);
        assert_eq!(pieces, all);
        assert_eq!(Plant::new(config, 60_000).level(), whole.level());
    }

    #[test]
    fn turning_back_undoes_the_flips_in_reverse() {
        let config = PlantConfig {
            seed: 3,
            double_permille: 500,
            ..PlantConfig::default()
        };
        let mut plant = Plant::new(config, 0);
        let forward = run(&mut plant, 30_000, Turn::Forward);
        let back = run(&mut plant, 30_000, Turn::Back);
        assert_eq!(plant.position_ms(), 0);
        assert!(plant.level());

        // The same flips, last first, each to the level before it, one
        // millisecond earlier as the rotor leaves its position.
        assert_eq!(forward.len(), back.len());
        for ((at, flip), (back_at, undone)) in forward.iter().rev().zip(&back) {
            assert_eq!(*back_at, at - 1);
            assert_eq!(undone.high, !flip.high);
        }

        // Behind position 0: the mark before it, first its rise, then its
        // fall, read backwards; and rebuilt there, the same level.
        let config = PlantConfig::default();
        let mut plant = Plant::new(config, 0);
        let behind = run(&mut plant, 9_000, Turn::Back);
        assert_eq!(behind.len(), 2, "{behind:?}");
        assert!(!behind[0].1.high && behind[0].0 == -1);
        assert!(behind[1].1.high && (-1202..=-1172).contains(&behind[1].0));
        assert!(plant.level());
        assert!(!Plant::new(config, -600).level());
        let far = run(&mut plant, 40_000, Turn::Back);
        assert_eq!(far.len(), 10, "five marks");
        assert_eq!(Plant::new(config, -49_000).level(), plant.level());
    }

    #[test]
    fn misses_and_bounces_follow_their_rates_and_the_seed() {
        let config = PlantConfig {
            seed: 42,
            missed_permille: 100,
            double_permille: 100,
            ..PlantConfig::default()
        };
        let flips = run(&mut Plant::new(config, 0), 9_074 * 400, Turn::Forward);
        let missed = flips.iter().filter(|(_, f)| !f.captured).count();
        let transitions = 800;
        // A bounced transition adds two flips.
        let bounced = (flips.len() - transitions) / 2;
        assert!((40..=120).contains(&missed), "{missed}");
        assert!((50..=110).contains(&bounced), "{bounced}");

        // Levels alternate, so each mark still ends HIGH.
        for pair in flips.windows(2) {
            assert_ne!(pair[0].1.high, pair[1].1.high);
        }

        let other = PlantConfig { seed: 43, ..config };
        let again = |config| run(&mut Plant::new(config, 0), 9_074 * 400, Turn::Forward);
        assert_ne!(again(other), flips);
        assert_eq!(again(config), flips);
    }

    #[test]
    fn the_driving_current_ripples_as_the_feeds_read() {
        let config = PlantConfig::default();
        let scaled = |raw: u16| (u32::from(raw) * 10).saturating_sub(2480) * 1000 / 12050;
        let mut plant = Plant::new(config, 0);
        let readings: Vec<u32> = (0..27_000)
            .map(|_| {
                plant.drive(Turn::Forward);
                scaled(plant.drive_reading())
            })
            .collect();
        let max = readings.iter().copied().max().unwrap();
        let mean = readings.iter().sum::<u32>() / 27_000;
        assert!((160..=168).contains(&max), "{max}");
        assert!((56..=66).contains(&mean), "{mean}");
        // Never near the 300 limit: only the surge reaches it.
        assert!(max < 300);
    }

    #[test]
    fn a_plant_whose_transitions_overlap_is_refused() {
        for config in [
            PlantConfig {
                low_ms: 3,
                low_jitter_ms: 0,
                ..PlantConfig::default()
            },
            PlantConfig {
                period_ms: 1200,
                ..PlantConfig::default()
            },
            PlantConfig {
                missed_permille: 1001,
                ..PlantConfig::default()
            },
        ] {
            assert!(!config.valid(), "{config:?}");
        }
    }
}
