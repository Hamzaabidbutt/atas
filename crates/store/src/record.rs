//! On-disk tick record encoding.
//!
//! Records are fixed width and little-endian. Fixed width is the whole design:
//! because every record is the same size and segments are strictly ordered by
//! time, a range query can binary-search a segment by seeking to
//! `index * RECORD_SIZE` — no separate index file to build, keep in sync, or
//! lose to a crash.

use atas_core::{Price, Qty, Side, Trade, Ts};

/// Bytes per record.
///
/// 33 bytes of payload padded to 40 so records stay 8-byte aligned when a
/// segment is read into memory.
pub const RECORD_SIZE: usize = 40;

/// Bit set in the flag byte when the aggressor was a buyer.
const FLAG_AGGRESSOR_BUY: u8 = 0b0000_0001;

/// Bits that have a defined meaning. Anything else means the file was written
/// by a newer format than this build understands.
const KNOWN_FLAGS: u8 = FLAG_AGGRESSOR_BUY;

/// A record that could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// The flag byte carried bits this build does not define.
    #[error("record has unknown flag bits {0:#010b}")]
    UnknownFlags(u8),
}

/// Serialise a trade into its on-disk form.
pub fn encode(trade: &Trade) -> [u8; RECORD_SIZE] {
    let mut buf = [0u8; RECORD_SIZE];
    buf[0..8].copy_from_slice(&trade.ts.nanos().to_le_bytes());
    buf[8..16].copy_from_slice(&trade.price.minor().to_le_bytes());
    buf[16..24].copy_from_slice(&trade.qty.minor().to_le_bytes());
    buf[24..32].copy_from_slice(&trade.id.to_le_bytes());
    buf[32] = match trade.aggressor {
        Side::Buy => FLAG_AGGRESSOR_BUY,
        Side::Sell => 0,
    };
    // buf[33..40] stays zero: reserved padding.
    buf
}

/// Deserialise a trade from its on-disk form.
pub fn decode(buf: &[u8; RECORD_SIZE]) -> Result<Trade, RecordError> {
    let flags = buf[32];
    if flags & !KNOWN_FLAGS != 0 {
        return Err(RecordError::UnknownFlags(flags));
    }
    Ok(Trade {
        ts: Ts::from_nanos(i64::from_le_bytes(buf[0..8].try_into().unwrap())),
        price: Price::from_minor(i64::from_le_bytes(buf[8..16].try_into().unwrap())),
        qty: Qty::from_minor(i64::from_le_bytes(buf[16..24].try_into().unwrap())),
        id: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
        aggressor: if flags & FLAG_AGGRESSOR_BUY != 0 {
            Side::Buy
        } else {
            Side::Sell
        },
    })
}

/// Read just the timestamp out of a record.
///
/// Binary search only needs this field, and skipping the rest keeps the search
/// to one 8-byte read per probe.
#[inline]
pub fn decode_ts(buf: &[u8; RECORD_SIZE]) -> Ts {
    Ts::from_nanos(i64::from_le_bytes(buf[0..8].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(side: Side) -> Trade {
        Trade::new(
            Ts::from_millis(1_700_000_000_123),
            Price::parse("95123.45").unwrap(),
            Qty::parse("0.00123456").unwrap(),
            side,
            9_876_543_210,
        )
    }

    #[test]
    fn round_trips_both_sides() {
        for side in [Side::Buy, Side::Sell] {
            let original = trade(side);
            let decoded = decode(&encode(&original)).unwrap();
            assert_eq!(decoded, original, "{side:?}");
        }
    }

    #[test]
    fn round_trips_extreme_values() {
        let t = Trade::new(
            Ts::from_nanos(i64::MAX),
            Price::from_minor(i64::MAX),
            Qty::from_minor(i64::MIN),
            Side::Buy,
            u64::MAX,
        );
        assert_eq!(decode(&encode(&t)).unwrap(), t);
    }

    #[test]
    fn timestamp_shortcut_matches_a_full_decode() {
        let t = trade(Side::Buy);
        let buf = encode(&t);
        assert_eq!(decode_ts(&buf), decode(&buf).unwrap().ts);
    }

    #[test]
    fn padding_is_zeroed() {
        let buf = encode(&trade(Side::Buy));
        assert_eq!(&buf[33..40], &[0u8; 7], "reserved bytes must stay zero");
    }

    #[test]
    fn rejects_records_from_a_newer_format() {
        let mut buf = encode(&trade(Side::Buy));
        buf[32] |= 0b1000_0000;
        assert!(matches!(
            decode(&buf),
            Err(RecordError::UnknownFlags(0b1000_0001))
        ));
    }
}
