//! Stock's stored feeding plans read from a file: the two plan sectors on
//! their own, or a whole flash read that holds them. The decoder and the
//! conversion are the installer's (`wellfed_client::install::plans`), so
//! this reports exactly what an install would import.

use std::fmt::Write as _;

use wellfed_client::install::plans::{
    self, Import, Note, Outcome, PLAN_SECTORS, StockPlans, StockTime,
};

/// Bytes in a whole flash read of the feeder.
const FLASH_BYTES: usize = 0x80_0000;

/// Why a file holds no plan sectors to read.
#[derive(Debug, thiserror::Error)]
pub enum PlansError {
    #[error(
        "{len} bytes are neither stock's plan sectors ({sectors}) nor a whole flash read ({FLASH_BYTES})"
    )]
    Size { len: usize, sectors: usize },
    #[error(transparent)]
    Decode(#[from] plans::DecodeError),
}

/// Stock's plans from `bytes`: the plan sectors, or a whole flash read.
pub fn decode(bytes: &[u8]) -> Result<StockPlans, PlansError> {
    let sectors = PLAN_SECTORS.len();
    let window = match bytes.len() {
        FLASH_BYTES => bytes
            .get(PLAN_SECTORS.start as usize..PLAN_SECTORS.end as usize)
            .unwrap_or_default(),
        len if len == sectors => bytes,
        len => return Err(PlansError::Size { len, sectors }),
    };
    Ok(StockPlans::decode(window)?)
}

/// One line per stock plan: what it became, or why it was refused.
pub fn report(import: &Import) -> String {
    let mut out = String::new();
    if import.outcomes.is_empty() {
        out.push_str("Stock holds no plans.\n");
    }

    for outcome in &import.outcomes {
        let line = match outcome {
            Outcome::Converted {
                slot, stock, plan, ..
            } => format!(
                "slot {slot}: stock plan {} -> {:02}:{:02} {}, {} portions{}",
                stock.id,
                plan.minute / 60,
                plan.minute % 60,
                plan.days,
                plan.portions,
                if plan.enabled { "" } else { ", disabled" },
            ),
            Outcome::Refused {
                slot,
                stock,
                refusal,
            } => format!(
                "slot {slot}: stock plan {} ({}) refused: {refusal}",
                stock.id,
                StockTime {
                    hour: stock.hour,
                    minute: stock.minute
                },
            ),
        };
        let _ = writeln!(out, "{line}");

        for note in outcome.notes() {
            let note = match note {
                Note::CallDropped { times } => {
                    format!("its meal call ({times} plays) is not carried over")
                }
                Note::PendingDropped => {
                    "the meal stock had started or still owed is not run again; \
                     it runs from its next occurrence"
                        .to_owned()
                }
                Note::SkipDisabled { until_ms } => {
                    format!("skipped until {until_ms} ms, so it arrives disabled")
                }
            };
            let _ = writeln!(out, "  note: {note}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_client::install::plans::{StockPlan, StockPlanState, convert};

    fn plan(hour: u8, portions: u8) -> StockPlan {
        StockPlan {
            id: 42,
            hour,
            minute: 15,
            once: false,
            days: [1, 2, 0, 0, 0, 0, 0],
            call: true,
            call_times: 2,
            portions,
            state: StockPlanState::Idle,
            sync_ms: 0,
            skip_until_ms: 0,
        }
    }

    #[test]
    fn a_flash_read_or_the_sectors_alone_decode_the_same() {
        let mut stock = StockPlans::default();
        stock.slots[0] = Some(plan(8, 3));
        let sectors = stock.encode();

        let mut flash = vec![0xff; FLASH_BYTES];
        flash[PLAN_SECTORS.start as usize..PLAN_SECTORS.end as usize].copy_from_slice(&sectors);
        assert_eq!(decode(&flash).unwrap(), stock);
        assert_eq!(decode(&sectors).unwrap(), stock);
        assert!(matches!(
            decode(&sectors[1..]),
            Err(PlansError::Size { .. })
        ));
    }

    #[test]
    fn the_report_names_each_plan() {
        let mut stock = StockPlans::default();
        stock.slots[0] = Some(plan(8, 3));
        stock.slots[1] = Some(plan(8, 0));

        let report = report(&convert(&stock, 0));
        assert_eq!(
            report,
            "slot 0: stock plan 42 -> 08:15 mon,tue, 3 portions\n  \
             note: its meal call (2 plays) is not carried over\n\
             slot 1: stock plan 42 (08:15) refused: it asks for 0 portions, not 1 to 48\n"
        );
        assert_eq!(
            super::report(&convert(&StockPlans::default(), 0)),
            "Stock holds no plans.\n"
        );
    }
}
