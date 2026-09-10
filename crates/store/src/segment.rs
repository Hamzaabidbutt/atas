//! A single segment file: a contiguous, time-ordered run of tick records.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use atas_core::{Trade, Ts};

use crate::error::StoreError;
use crate::record::{self, RECORD_SIZE};

/// Filename for segment `index`, zero-padded so a directory listing sorts
/// in the same order as the segment sequence.
pub fn segment_name(index: u32) -> String {
    format!("seg-{index:06}.tick")
}

/// Parse a segment index back out of a filename.
pub fn parse_segment_name(name: &str) -> Option<u32> {
    name.strip_prefix("seg-")?
        .strip_suffix(".tick")?
        .parse()
        .ok()
}

/// One segment file on disk.
#[derive(Debug, Clone)]
pub struct Segment {
    path: PathBuf,
    index: u32,
    records: u64,
    first_ts: Ts,
    last_ts: Ts,
}

impl Segment {
    /// Open an existing segment, repairing a torn tail if one is present.
    ///
    /// A process killed mid-append leaves a partial record. Because records are
    /// fixed width, the damage is always confined to the final one, and
    /// truncating it back to a record boundary is a complete repair.
    pub fn open(path: PathBuf, index: u32) -> Result<Self, StoreError> {
        let len = std::fs::metadata(&path)?.len();
        let remainder = len % RECORD_SIZE as u64;

        if remainder != 0 {
            let whole = len - remainder;
            OpenOptions::new()
                .write(true)
                .open(&path)?
                .set_len(whole)?;
        }
        let records = len / RECORD_SIZE as u64;

        let mut segment = Self {
            path,
            index,
            records,
            first_ts: Ts::EPOCH,
            last_ts: Ts::EPOCH,
        };

        if records > 0 {
            let mut file = segment.open_file()?;
            segment.first_ts = segment.read_ts(&mut file, 0)?;
            segment.last_ts = segment.read_ts(&mut file, records - 1)?;
        }
        Ok(segment)
    }

    /// Create an empty segment file.
    pub fn create(path: PathBuf, index: u32) -> Result<Self, StoreError> {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            index,
            records: 0,
            first_ts: Ts::EPOCH,
            last_ts: Ts::EPOCH,
        })
    }

    /// Path to the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sequence number of this segment.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Number of complete records.
    pub fn len(&self) -> u64 {
        self.records
    }

    /// Whether the segment holds no records.
    pub fn is_empty(&self) -> bool {
        self.records == 0
    }

    /// Timestamp of the first record.
    pub fn first_ts(&self) -> Ts {
        self.first_ts
    }

    /// Timestamp of the last record.
    pub fn last_ts(&self) -> Ts {
        self.last_ts
    }

    /// Whether this segment could hold records in `[from, to)`.
    pub fn overlaps(&self, from: Ts, to: Ts) -> bool {
        !self.is_empty() && self.first_ts < to && self.last_ts >= from
    }

    /// Account for records appended by the store's writer.
    pub(crate) fn note_appended(&mut self, count: u64, first_ts: Ts, last_ts: Ts) {
        if self.records == 0 {
            self.first_ts = first_ts;
        }
        self.records += count;
        self.last_ts = last_ts;
    }

    fn open_file(&self) -> Result<File, StoreError> {
        Ok(File::open(&self.path)?)
    }

    /// Read a single record by position, or `None` if out of range.
    pub fn record_at(&self, index: u64) -> Result<Option<Trade>, StoreError> {
        if index >= self.records {
            return Ok(None);
        }
        let mut file = self.open_file()?;
        let mut buf = [0u8; RECORD_SIZE];
        file.seek(SeekFrom::Start(index * RECORD_SIZE as u64))?;
        file.read_exact(&mut buf)?;
        Ok(Some(record::decode(&buf)?))
    }

    fn read_ts(&self, file: &mut File, index: u64) -> Result<Ts, StoreError> {
        let mut buf = [0u8; RECORD_SIZE];
        file.seek(SeekFrom::Start(index * RECORD_SIZE as u64))?;
        file.read_exact(&mut buf)?;
        Ok(record::decode_ts(&buf))
    }

    /// Index of the first record with a timestamp at or after `ts`.
    ///
    /// Returns [`Segment::len`] when every record predates `ts`.
    pub fn lower_bound(&self, ts: Ts) -> Result<u64, StoreError> {
        if self.records == 0 || ts <= self.first_ts {
            return Ok(0);
        }
        if ts > self.last_ts {
            return Ok(self.records);
        }

        let mut file = self.open_file()?;
        let (mut lo, mut hi) = (0u64, self.records);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.read_ts(&mut file, mid)? < ts {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// Read records from `start` while their timestamp is below `until`.
    ///
    /// Returns `true` if it stopped because a record reached `until`, meaning
    /// no later segment can contribute either.
    pub fn scan_from(
        &self,
        start: u64,
        until: Ts,
        visit: &mut impl FnMut(Trade),
    ) -> Result<bool, StoreError> {
        if start >= self.records {
            return Ok(false);
        }

        let mut file = self.open_file()?;
        file.seek(SeekFrom::Start(start * RECORD_SIZE as u64))?;
        // 64 KiB of records per syscall rather than one seek+read each.
        let mut reader = BufReader::with_capacity(RECORD_SIZE * 1638, file);

        let mut buf = [0u8; RECORD_SIZE];
        for _ in start..self.records {
            reader.read_exact(&mut buf)?;
            if record::decode_ts(&buf) >= until {
                return Ok(true);
            }
            visit(record::decode(&buf)?);
        }
        Ok(false)
    }
}
