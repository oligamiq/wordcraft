//! Trusted-coordinator page collector for incremental plain-body layout.
//!
//! This is an in-memory state machine. External durability, atomic CAS and
//! authenticated producer provenance must be added by the service integration.
use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::{BlockDeltaCheckpoint, ContinuationError, PageDeltaStep, Phase, decode};
use crate::{DocLayout, Page, pass, wire};

const MAX_PAGES: usize = 10_000;
const MAX_TOTAL_WIRE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECEIPTS: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorAcceptance {
    Applied,
    Duplicate,
}

/// Collects complete typed pages in order. Existing committed transitions
/// can be retried, but a different response for that predecessor is rejected.
pub struct PageDeltaCollector {
    source_digest: [u8; 32],
    options_digest: [u8; 32],
    block_limit: u32,
    pages: Vec<Page>,
    current_checkpoint: Option<Vec<u8>>,
    receipts: BTreeMap<[u8; 32], [u8; 32]>,
    last_block: u32,
    generation: u64,
    total_wire_bytes: usize,
    complete: bool,
}

impl PageDeltaCollector {
    pub fn new(source_digest: [u8; 32], options_digest: [u8; 32], block_limit: u32) -> Self {
        Self {
            source_digest,
            options_digest,
            block_limit,
            pages: Vec::new(),
            current_checkpoint: None,
            receipts: BTreeMap::new(),
            last_block: 0,
            generation: 0,
            total_wire_bytes: 0,
            complete: false,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn checkpoint(&self) -> Option<&[u8]> {
        self.current_checkpoint.as_deref()
    }

    /// Only a completed, ordered collection may become the final layout.
    pub fn into_layout(self) -> Result<DocLayout, ContinuationError> {
        if !self.complete {
            return Err(ContinuationError::InvalidState);
        }
        Ok(DocLayout { index: pass::index_lines(&self.pages), pages: self.pages, ..Default::default() })
    }

    /// Apply a trusted Worker result atomically. The predecessor is the exact
    /// checkpoint that was supplied to that Worker call (None for first call).
    pub fn apply(&mut self, predecessor: Option<&[u8]>, result: &PageDeltaStep) -> Result<CollectorAcceptance, ContinuationError> {
        let key = predecessor_key(predecessor);
        let value = response_digest(result);
        if let Some(known) = self.receipts.get(&key) {
            return if *known == value { Ok(CollectorAcceptance::Duplicate) } else { Err(ContinuationError::InvalidState) };
        }
        if self.complete || predecessor != self.current_checkpoint.as_deref() || self.block_limit == 0 {
            return Err(ContinuationError::InvalidState);
        }
        if self.receipts.len() >= MAX_RECEIPTS {
            return Err(ContinuationError::OutOfBounds);
        }

        let (new_pages, next_checkpoint, next_block, is_complete, input_bytes) = match result {
            PageDeltaStep::Continue { sealed_page_wire, checkpoint, first_page } => {
                if *first_page != self.pages.len() {
                    return Err(ContinuationError::InvalidState);
                }
                let env = decode(checkpoint, self.source_digest, self.options_digest, 1, self.block_limit)?;
                if env.cursor.phase != Phase::Paginating
                    || env.cursor.section != 0
                    || env.cursor.iteration != 0
                    || env.cursor.block <= self.last_block
                    || env.cursor.block >= self.block_limit
                {
                    return Err(ContinuationError::InvalidState);
                }
                let state: BlockDeltaCheckpoint = serde_json::from_slice(&env.payload).map_err(|_| ContinuationError::InvalidState)?;
                if state.schema != 2
                    || state.state.next_block != env.cursor.block as usize
                    || state.state.pages.len() != 1
                    || state.state.behind.len() != 1
                {
                    return Err(ContinuationError::InvalidState);
                }
                let sealed = match sealed_page_wire {
                    Some(bytes) => {
                        let pages = decode_pages(bytes)?;
                        if pages.is_empty() {
                            return Err(ContinuationError::InvalidState);
                        }
                        pages
                    }
                    None => Vec::new(),
                };
                let expected = self.pages.len().checked_add(sealed.len()).ok_or(ContinuationError::OutOfBounds)?;
                if state.state.page_base != expected {
                    return Err(ContinuationError::InvalidState);
                }
                (sealed, Some(checkpoint.clone()), env.cursor.block, false, sealed_page_wire.as_ref().map_or(0, Vec::len))
            }
            PageDeltaStep::Complete { tail_wire, first_page } => {
                if *first_page != self.pages.len() {
                    return Err(ContinuationError::InvalidState);
                }
                let pages = decode_pages(tail_wire)?;
                if pages.is_empty() {
                    return Err(ContinuationError::InvalidState);
                }
                (pages, None, self.block_limit, true, tail_wire.len())
            }
        };

        let page_count = self.pages.len().checked_add(new_pages.len()).ok_or(ContinuationError::OutOfBounds)?;
        let wire_bytes = self.total_wire_bytes.checked_add(input_bytes).ok_or(ContinuationError::OutOfBounds)?;
        if page_count > MAX_PAGES || wire_bytes > MAX_TOTAL_WIRE_BYTES {
            return Err(ContinuationError::OutOfBounds);
        }
        if new_pages.iter().any(|p| !p.w.is_finite() || !p.h.is_finite() || p.w <= 0.0 || p.h <= 0.0 || p.section != 0 || p.number == 0) {
            return Err(ContinuationError::InvalidState);
        }

        let next_generation = self.generation.checked_add(1).ok_or(ContinuationError::OutOfBounds)?;
        // Commit as one mutation after validation; failed delivery changes nothing.
        self.pages.extend(new_pages);
        self.current_checkpoint = next_checkpoint;
        self.last_block = next_block;
        self.total_wire_bytes = wire_bytes;
        self.complete = is_complete;
        self.generation = next_generation;
        self.receipts.insert(key, value);
        Ok(CollectorAcceptance::Applied)
    }
}

fn decode_pages(bytes: &[u8]) -> Result<Vec<Page>, ContinuationError> {
    if bytes.len() > wire::MAX_WIRE_BYTES {
        return Err(ContinuationError::PayloadSize);
    }
    wire::decode(bytes).map(|l| l.pages).map_err(|_| ContinuationError::Malformed)
}

fn hash_chunk(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn predecessor_key(predecessor: Option<&[u8]>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"FVDL/previous/v1");
    match predecessor {
        None => h.update([0]),
        Some(bytes) => {
            h.update([1]);
            hash_chunk(&mut h, bytes);
        }
    }
    h.finalize().into()
}

fn response_digest(response: &PageDeltaStep) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"FVDL/response/v1");
    match response {
        PageDeltaStep::Continue { sealed_page_wire, checkpoint, first_page } => {
            h.update([1]);
            h.update((*first_page as u64).to_le_bytes());
            hash_chunk(&mut h, checkpoint);
            match sealed_page_wire {
                None => h.update([0]),
                Some(bytes) => {
                    h.update([1]);
                    hash_chunk(&mut h, bytes);
                }
            }
        }
        PageDeltaStep::Complete { tail_wire, first_page } => {
            h.update([2]);
            h.update((*first_page as u64).to_le_bytes());
            hash_chunk(&mut h, tail_wire);
        }
    }
    h.finalize().into()
}

impl PageDeltaCollector {
    /// Encodes one immutable commit candidate for a durable append-only store.
    /// The actual storage system must atomically compare its generation and
    /// append this record; an in-memory generation is NOT distributed CAS.
    pub fn encode_journal_record(&self, predecessor: Option<&[u8]>, step: &PageDeltaStep) -> Result<Vec<u8>, ContinuationError> {
        super::journal::encode(self.generation, self.source_digest, self.options_digest, predecessor, step)
    }

    /// Replays one authenticated-by-the-caller record and checks its revision.
    /// Identical historical replays are read-only; stale divergent writes fail.
    pub fn apply_journal_record(&mut self, bytes: &[u8]) -> Result<CollectorAcceptance, ContinuationError> {
        let record = super::journal::decode(bytes, self.source_digest, self.options_digest)?;
        if record.generation != self.generation {
            let previous_key = predecessor_key(record.predecessor.as_deref());
            let response_key = response_digest(&record.step);
            return if self.receipts.get(&previous_key) == Some(&response_key) {
                Ok(CollectorAcceptance::Duplicate)
            } else {
                Err(ContinuationError::InvalidState)
            };
        }
        self.apply(record.predecessor.as_deref(), &record.step)
    }

    /// Rebuild from unique ordered immutable records read from a trusted store.
    /// Reopening a process does not imply the store's append/CAS was atomic.
    pub fn replay_journal(
        source_digest: [u8; 32],
        options_digest: [u8; 32],
        block_limit: u32,
        records: &[Vec<u8>],
    ) -> Result<Self, ContinuationError> {
        if records.len() > MAX_RECEIPTS {
            return Err(ContinuationError::OutOfBounds);
        }
        let mut out = Self::new(source_digest, options_digest, block_limit);
        for record in records {
            if out.apply_journal_record(record)? != CollectorAcceptance::Applied {
                return Err(ContinuationError::InvalidState);
            }
        }
        Ok(out)
    }
}
