//! The tick store: append-only, segmented, time-ordered trade history.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use atas_core::{Instrument, Trade, Ts};

use crate::error::StoreError;
use crate::record::{self, RECORD_SIZE};
use crate::segment::{parse_segment_name, segment_name, Segment};

/// On-disk format version, written to the marker file at creation.
pub const FORMAT_VERSION: u32 = 1;

/// Marker file naming the directory as a tick store and pinning its format.
const MARKER: &str = ".atas-tickstore";

/// Records per segment before rolling to a new file.
///
/// A million 40-byte records is a 40 MB file: small enough that a corrupt
/// segment loses a bounded amount of history, large enough that a busy day
/// does not sprawl into thousands of files.
pub const DEFAULT_SEGMENT_RECORDS: u64 = 1_000_000;

/// Append-only tick history for one instrument.
///
/// # Guarantees
///
/// - Records are stored in non-decreasing timestamp order. Out-of-order
///   appends are rejected rather than accepted and silently unsearchable.
/// - A crash mid-append loses at most the records still in the write buffer,
///   and any torn final record is repaired on the next open.
/// - Range queries are `O(log n)` seeks plus a sequential read of the matched
///   span, with no index file to corrupt or rebuild.
#[derive(Debug)]
pub struct TickStore {
    root: PathBuf,
    segments: Vec<Segment>,
    writer: Option<BufWriter<File>>,
    segment_records: u64,
    last_ts: Ts,
    total: u64,
}

impl TickStore {
    /// Open the store for an instrument beneath `root`, creating it if needed.
    ///
    /// History lives at `<root>/<venue>/<symbol>/`.
    pub fn open(root: impl AsRef<Path>, instrument: &Instrument) -> Result<Self, StoreError> {
        let dir = root
            .as_ref()
            .join(instrument.venue.as_str())
            .join(&instrument.symbol);
        Self::open_dir(dir)
    }

    /// Open the store rooted at an exact directory.
    pub fn open_dir(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = dir.into();
        fs::create_dir_all(&root)?;
        Self::check_marker(&root)?;

        let mut segments = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(index) = parse_segment_name(name) else {
                continue;
            };
            segments.push(Segment::open(entry.path(), index)?);
        }
        segments.sort_by_key(|s| s.index());

        // A crash can leave an empty trailing segment; it is harmless but
        // dropping it keeps `last_ts` derivation simple.
        while segments.last().is_some_and(|s| s.is_empty()) {
            let empty = segments.pop().expect("checked non-empty");
            fs::remove_file(empty.path())?;
        }

        let total = segments.iter().map(|s| s.len()).sum();
        let last_ts = segments.last().map(|s| s.last_ts()).unwrap_or(Ts::EPOCH);

        Ok(Self {
            root,
            segments,
            writer: None,
            segment_records: DEFAULT_SEGMENT_RECORDS,
            last_ts,
            total,
        })
    }

    /// Read the marker file, or write it if the directory is new.
    fn check_marker(root: &Path) -> Result<(), StoreError> {
        let marker = root.join(MARKER);
        if marker.exists() {
            let text = fs::read_to_string(&marker)?;
            let found: u32 = text
                .trim()
                .parse()
                .map_err(|_| StoreError::NotAStore(root.to_path_buf()))?;
            if found != FORMAT_VERSION {
                return Err(StoreError::UnsupportedVersion {
                    found,
                    expected: FORMAT_VERSION,
                });
            }
            return Ok(());
        }

        // Refuse to claim a directory that already holds unrelated files.
        let occupied = fs::read_dir(root)?.next().is_some();
        if occupied {
            return Err(StoreError::NotAStore(root.to_path_buf()));
        }
        fs::write(marker, FORMAT_VERSION.to_string())?;
        Ok(())
    }

    /// Override how many records a segment holds before rolling.
    pub fn with_segment_records(mut self, records: u64) -> Self {
        assert!(records > 0, "segment size must be positive");
        self.segment_records = records;
        self
    }

    /// Directory backing this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Total records stored, including any still buffered.
    pub fn len(&self) -> u64 {
        self.total
    }

    /// Whether any record has been stored.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Number of segment files.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Timestamp of the earliest stored record.
    pub fn first_ts(&self) -> Option<Ts> {
        self.segments.first().map(|s| s.first_ts())
    }

    /// Timestamp of the latest stored record.
    pub fn last_ts(&self) -> Option<Ts> {
        (!self.is_empty()).then_some(self.last_ts)
    }

    /// Append one trade.
    ///
    /// Prefer [`TickStore::append_batch`] on a live feed: it takes the
    /// segment-roll check once for the batch instead of once per trade.
    pub fn append(&mut self, trade: &Trade) -> Result<(), StoreError> {
        self.append_batch(std::slice::from_ref(trade))
    }

    /// Append a batch of trades, which must be in non-decreasing time order.
    ///
    /// The batch is validated before anything is written, so a bad batch
    /// leaves the store exactly as it was rather than half-applied.
    pub fn append_batch(&mut self, trades: &[Trade]) -> Result<(), StoreError> {
        if trades.is_empty() {
            return Ok(());
        }

        let mut previous = self.last_ts;
        let has_history = !self.is_empty();
        for (i, trade) in trades.iter().enumerate() {
            if (has_history || i > 0) && trade.ts < previous {
                return Err(StoreError::OutOfOrder {
                    received: trade.ts,
                    last: previous,
                });
            }
            previous = trade.ts;
        }

        let mut written = 0usize;
        while written < trades.len() {
            self.ensure_writer()?;
            let current = self.segments.last().expect("ensure_writer created one");
            let room = self.segment_records.saturating_sub(current.len()) as usize;
            debug_assert!(room > 0, "ensure_writer must leave room");

            let take = room.min(trades.len() - written);
            let chunk = &trades[written..written + take];

            let writer = self.writer.as_mut().expect("ensure_writer created one");
            for trade in chunk {
                writer.write_all(&record::encode(trade))?;
            }

            let segment = self.segments.last_mut().expect("ensure_writer created one");
            segment.note_appended(
                chunk.len() as u64,
                chunk[0].ts,
                chunk[chunk.len() - 1].ts,
            );

            self.total += chunk.len() as u64;
            self.last_ts = chunk[chunk.len() - 1].ts;
            written += take;

            if segment.len() >= self.segment_records {
                self.close_writer()?;
            }
        }
        Ok(())
    }

    /// Make sure there is a writable segment with room in it.
    fn ensure_writer(&mut self) -> Result<(), StoreError> {
        let needs_new = match self.segments.last() {
            None => true,
            Some(s) => s.len() >= self.segment_records,
        };

        if needs_new {
            self.close_writer()?;
            let index = self.segments.last().map_or(0, |s| s.index() + 1);
            let path = self.root.join(segment_name(index));
            self.segments.push(Segment::create(path, index)?);
        }

        if self.writer.is_none() {
            let path = self.segments.last().expect("segment exists").path();
            let file = OpenOptions::new().append(true).open(path)?;
            self.writer = Some(BufWriter::with_capacity(RECORD_SIZE * 1638, file));
        }
        Ok(())
    }

    fn close_writer(&mut self) -> Result<(), StoreError> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
        }
        Ok(())
    }

    /// Flush buffered records so readers and other processes can see them.
    ///
    /// This does not fsync. Use [`TickStore::sync`] when durability across a
    /// machine crash matters, not merely across a process crash.
    pub fn flush(&mut self) -> Result<(), StoreError> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }

    /// Flush and fsync the current segment.
    pub fn sync(&mut self) -> Result<(), StoreError> {
        self.flush()?;
        if let Some(writer) = self.writer.as_mut() {
            writer.get_ref().sync_data()?;
        }
        Ok(())
    }

    /// Visit every stored trade in `[from, to)` in time order.
    ///
    /// Streaming rather than collecting: a day of a busy instrument is tens of
    /// millions of records, and materialising that into a `Vec` to compute a
    /// profile over it would be gratuitous.
    pub fn scan(
        &mut self,
        from: Ts,
        to: Ts,
        mut visit: impl FnMut(Trade),
    ) -> Result<(), StoreError> {
        if from >= to {
            return Ok(());
        }
        // Buffered records are part of the history being queried.
        self.flush()?;

        for segment in &self.segments {
            if !segment.overlaps(from, to) {
                // Segments are time-ordered, so once one starts at or after
                // `to`, no later segment can contribute.
                if segment.first_ts() >= to {
                    break;
                }
                continue;
            }
            let start = segment.lower_bound(from)?;
            if segment.scan_from(start, to, &mut visit)? {
                break;
            }
        }
        Ok(())
    }

    /// Collect the trades in `[from, to)`.
    ///
    /// Convenient for tests and bounded queries; prefer [`TickStore::scan`]
    /// for anything that could match a large span.
    pub fn read(&mut self, from: Ts, to: Ts) -> Result<Vec<Trade>, StoreError> {
        let mut out = Vec::new();
        self.scan(from, to, |t| out.push(t))?;
        Ok(out)
    }

    /// Every stored trade.
    pub fn read_all(&mut self) -> Result<Vec<Trade>, StoreError> {
        match (self.first_ts(), self.last_ts()) {
            (Some(first), Some(last)) => self.read(first, last + 1),
            _ => Ok(Vec::new()),
        }
    }
}

impl Drop for TickStore {
    fn drop(&mut self) {
        // A dropped store must not lose buffered records. Errors here cannot
        // be reported, which is exactly why `flush` is also public.
        let _ = self.flush();
    }
}
