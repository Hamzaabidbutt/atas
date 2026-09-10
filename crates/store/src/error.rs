//! Storage errors.

use atas_core::Ts;

use crate::record::RecordError;

/// Anything that can go wrong reading or writing tick history.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Underlying filesystem failure.
    #[error("tick store I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A record on disk could not be decoded.
    #[error("corrupt tick record: {0}")]
    Record(#[from] RecordError),

    /// An append arrived with a timestamp before the last stored record.
    ///
    /// The store refuses it because binary-search range queries depend on
    /// records being ordered; accepting it would silently break every later
    /// read of this instrument's history.
    #[error("trade at {received} is older than the last stored trade at {last}")]
    OutOfOrder {
        /// Timestamp of the rejected trade.
        received: Ts,
        /// Timestamp already on disk.
        last: Ts,
    },

    /// The directory holds data written by an incompatible format version.
    #[error("tick store format version {found} is not supported (this build writes {expected})")]
    UnsupportedVersion {
        /// Version read from the marker file.
        found: u32,
        /// Version this build writes.
        expected: u32,
    },

    /// The directory exists but is not a tick store.
    #[error("{0} exists but is not a tick store directory")]
    NotAStore(std::path::PathBuf),
}
