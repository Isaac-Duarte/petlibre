//! What one portion weighs, from a scale's reading of counted portions
//! ([calibration]): grams as a person types them, and the mean the feeder
//! stores. The feeder checks the 1-50 g limit itself.
//!
//! [calibration]: ../../../../docs/firmware/calibration.md

/// A weight in grams that does not parse.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{text:?} is not a weight in grams (like 9.8)")]
pub struct NotGrams {
    text: String,
}

/// Grams with up to three decimals, as milligrams.
pub fn parse_grams(text: &str) -> Result<u32, NotGrams> {
    let invalid = || NotGrams {
        text: text.to_owned(),
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() || fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole: u32 = whole.parse().map_err(|_| invalid())?;
    let padded = format!("{fraction:0<3}");
    let fraction: u32 = padded.parse().map_err(|_| invalid())?;
    whole
        .checked_mul(1000)
        .and_then(|mg| mg.checked_add(fraction))
        .ok_or_else(invalid)
}

/// A mean over no portions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("count at least one portion")]
pub struct NoPortions;

/// The mean of `portions` portions that weighed `weighed_mg` together,
/// rounded to the milligram.
pub fn mean_mg(weighed_mg: u32, portions: u32) -> Result<u32, NoPortions> {
    if portions == 0 {
        return Err(NoPortions);
    }
    let mean = (u64::from(weighed_mg) + u64::from(portions) / 2) / u64::from(portions);
    // At most `weighed_mg`, and so a u32, with one portion or more.
    Ok(u32::try_from(mean).unwrap_or(weighed_mg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_parse_to_milligrams() {
        assert_eq!(parse_grams("9.8"), Ok(9_800));
        assert_eq!(parse_grams("98.425"), Ok(98_425));
        assert_eq!(parse_grams("12"), Ok(12_000));
        for bad in ["", ".5", "9.8765", "9,8", "-1", "1e3", "9.x"] {
            assert!(parse_grams(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_mean_rounds_and_needs_a_portion() {
        assert_eq!(mean_mg(98_400, 10), Ok(9_840));
        assert_eq!(mean_mg(10_000, 3), Ok(3_333));
        assert_eq!(mean_mg(u32::MAX, 1), Ok(u32::MAX));
        assert_eq!(mean_mg(1_000, 0), Err(NoPortions));
    }
}
