//! Stock's stored feeding plans read from a file: the two plan sectors on
//! their own, a whole flash read that holds them, or the copy an install
//! kept beside the stock backup. The decoder and the
//! conversion are the installer's (`wellfed_client::install::plans`), so
//! this reports exactly what an install would import.

use wellfed_client::install::plans::{self, KEPT_LEN, PLAN_SECTORS, StockPlans};

/// Bytes in a whole flash read of the feeder.
pub(crate) const FLASH_BYTES: usize = 0x80_0000;

/// Why a file holds no plan sectors to read.
#[derive(Debug, thiserror::Error)]
pub enum PlansError {
    #[error(
        "{len} bytes are neither stock's plan sectors ({sectors}), an install's kept copy ({KEPT_LEN}) nor a whole flash read ({FLASH_BYTES})"
    )]
    Size { len: usize, sectors: usize },
    #[error(transparent)]
    Decode(#[from] plans::DecodeError),
}

/// Stock's plans from `bytes`: the plan sectors, an install's kept copy,
/// or a whole flash read.
pub fn decode(bytes: &[u8]) -> Result<StockPlans, PlansError> {
    let sectors = PLAN_SECTORS.len();
    if bytes.len() == KEPT_LEN {
        return Ok(StockPlans::decode_kept(bytes)?);
    }
    let window = match bytes.len() {
        FLASH_BYTES => bytes
            .get(PLAN_SECTORS.start as usize..PLAN_SECTORS.end as usize)
            .unwrap_or_default(),
        len if len == sectors => bytes,
        len => return Err(PlansError::Size { len, sectors }),
    };
    Ok(StockPlans::decode(window)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_client::install::plans::{StockPlan, StockPlanState};

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
        let kept = plans::kept(sectors[0], &sectors[0x1000..0x1000 + plans::ENTRIES_LEN]);
        assert_eq!(decode(&kept).unwrap(), stock);
        assert!(matches!(
            decode(&sectors[1..]),
            Err(PlansError::Size { .. })
        ));
    }
}
