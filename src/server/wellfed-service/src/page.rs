//! `{ items, next }` pages over the lists that grow: a page's size, and the
//! opaque cursors its `next` carries.

use jiff::Timestamp;
use wellfed_api::{PageQuery, WeightId};
use wellfed_store::WeightCursor;
use uuid::Uuid;

use crate::Invalid;

const PAGE: u32 = 100;
const PAGE_MAX: u32 = 1000;

/// SQLite's largest integer, the largest `seq` a cursor can name.
const SEQ_MAX: u64 = i64::MAX.unsigned_abs();

/// A page's size, from `?limit=`.
pub(crate) fn limit(query: &PageQuery) -> Result<u32, Invalid> {
    let limit = query.limit.unwrap_or(PAGE);
    if !(1..=PAGE_MAX).contains(&limit) {
        return Err(Invalid::PageLimit);
    }
    Ok(limit)
}

/// The rows after a list's `seq` cursor, which is the number as text.
pub(crate) fn after_seq(query: &PageQuery) -> Result<Option<u64>, Invalid> {
    query
        .after
        .as_deref()
        .map(|after| match after.parse::<u64>() {
            Ok(after) if after <= SEQ_MAX => Ok(after),
            _ => Err(Invalid::PageCursor),
        })
        .transpose()
}

/// The readings after a weight log's cursor: the last reading's time in
/// Unix milliseconds and its id, as `<ms>.<id>`.
pub(crate) fn after_weight(query: &PageQuery) -> Result<Option<WeightCursor>, Invalid> {
    let Some(after) = query.after.as_deref() else {
        return Ok(None);
    };

    let (ms, id) = after.split_once('.').ok_or(Invalid::PageCursor)?;
    let at = ms
        .parse::<i64>()
        .ok()
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .ok_or(Invalid::PageCursor)?;
    let id = Uuid::try_parse(id).map_err(|_| Invalid::PageCursor)?;
    Ok(Some(WeightCursor {
        at,
        id: WeightId(id),
    }))
}

pub(crate) fn weight_cursor(at: Timestamp, id: WeightId) -> String {
    format!("{}.{}", at.as_millisecond(), id.0.simple())
}

/// Split one row more than `limit` off `rows`, which shows whether
/// another page follows, and name the last row kept as `next`.
pub(crate) fn split<T>(
    mut rows: Vec<T>,
    limit: u32,
    cursor: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let more = rows.len() > limit;
    rows.truncate(limit);

    let next = more.then(|| rows.last().map(&cursor)).flatten();
    (rows, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(after: &str) -> PageQuery {
        PageQuery {
            after: Some(after.into()),
            limit: None,
        }
    }

    #[test]
    fn a_weight_cursor_round_trips_and_anything_else_is_refused() {
        let at = Timestamp::from_millisecond(1_790_000_000_123).unwrap();
        let id = WeightId(Uuid::from_u128(0x0192_c5d0_aaaa));

        let cursor = weight_cursor(at, id);
        assert_eq!(
            after_weight(&query(&cursor)),
            Ok(Some(WeightCursor { at, id }))
        );
        for bad in ["", "7", "x.y", "1.zz", "99999999999999999999.0192c5d0"] {
            assert_eq!(after_weight(&query(bad)), Err(Invalid::PageCursor), "{bad}");
        }
    }

    #[test]
    fn a_seq_cursor_is_a_number_sqlite_holds() {
        assert_eq!(after_seq(&query("12")), Ok(Some(12)));
        for bad in ["-1", "x", "9223372036854775808"] {
            assert_eq!(after_seq(&query(bad)), Err(Invalid::PageCursor), "{bad}");
        }
    }

    #[test]
    fn a_page_names_its_last_row_only_when_more_follow() {
        let cursor = |n: &u32| n.to_string();

        assert_eq!(
            split(vec![1, 2, 3], 2, cursor),
            (vec![1, 2], Some("2".into()))
        );
        assert_eq!(split(vec![1, 2], 2, cursor), (vec![1, 2], None));
        assert_eq!(
            limit(&PageQuery {
                after: None,
                limit: Some(0)
            }),
            Err(Invalid::PageLimit)
        );
    }
}
