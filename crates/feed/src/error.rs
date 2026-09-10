//! Feed errors.

/// Anything that can go wrong translating a venue's wire format.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// The payload was not valid JSON.
    #[error("malformed JSON from feed: {0}")]
    Json(#[from] serde_json::Error),

    /// A field the adapter depends on was missing.
    #[error("missing field {field:?} in {context}")]
    MissingField {
        /// Name of the absent field.
        field: &'static str,
        /// Which message it was absent from.
        context: &'static str,
    },

    /// A field was present but could not be interpreted.
    #[error("field {field:?} in {context} has unexpected value {value:?}")]
    BadField {
        /// Name of the offending field.
        field: &'static str,
        /// Which message it came from.
        context: &'static str,
        /// The value as received.
        value: String,
    },

    /// A numeric field could not be parsed as a fixed-point decimal.
    #[error("field {field:?} in {context}: {source}")]
    BadNumber {
        /// Name of the offending field.
        field: &'static str,
        /// Which message it came from.
        context: &'static str,
        /// The underlying parse failure.
        #[source]
        source: atas_core::ParseFixedError,
    },

    /// The message was well-formed but is not one this adapter handles.
    /// Callers should skip it rather than treat it as a failure.
    #[error("unhandled message kind {0:?}")]
    Unhandled(String),

    /// Reading recorded or stored history failed.
    #[error("replay source error: {0}")]
    Store(#[from] atas_store::StoreError),
}
