//! Portable, append-only per-transition page-collector journal wire format.
//! A journal needs externally durable atomic append/CAS; this module supplies
//! validated records and replay, not storage or producer authentication.
use super::{ContinuationError, PageDeltaStep, digest, read_u32};

const MAGIC: &[u8; 4] = b"FVDJ";
const VERSION: u8 = 1;
const PREFIX: usize = 94;
const HEADER: usize = PREFIX + 32;
const MAX_RECORD: usize = 96 * 1024 * 1024;

pub(super) struct Record {
    pub(super) generation: u64,
    pub(super) predecessor: Option<Vec<u8>>,
    pub(super) step: PageDeltaStep,
}

pub(super) fn encode(
    generation: u64,
    source: [u8; 32],
    options: [u8; 32],
    predecessor: Option<&[u8]>,
    step: &PageDeltaStep,
) -> Result<Vec<u8>, ContinuationError> {
    let prev = predecessor.unwrap_or(&[]);
    if predecessor.is_some_and(|p| p.is_empty()) {
        return Err(ContinuationError::Malformed);
    }
    let (kind, first_page, checkpoint, wire) = match step {
        PageDeltaStep::Continue { sealed_page_wire, checkpoint, first_page } => {
            (1u8, *first_page, checkpoint.as_slice(), sealed_page_wire.as_deref().unwrap_or(&[]))
        }
        PageDeltaStep::Complete { tail_wire, first_page } => (2u8, *first_page, &[][..], tail_wire.as_slice()),
    };
    if (kind == 1 && checkpoint.is_empty()) || (kind == 2 && wire.is_empty()) {
        return Err(ContinuationError::Malformed);
    }
    let page = u32::try_from(first_page).map_err(|_| ContinuationError::OutOfBounds)?;
    let lengths = [prev.len(), checkpoint.len(), wire.len()];
    let body_len = lengths.into_iter().try_fold(0usize, |acc, n| acc.checked_add(n)).ok_or(ContinuationError::OutOfBounds)?;
    if body_len.checked_add(HEADER).is_none_or(|n| n > MAX_RECORD) {
        return Err(ContinuationError::PayloadSize);
    }
    let mut prefix = Vec::with_capacity(PREFIX);
    prefix.extend_from_slice(MAGIC);
    prefix.push(VERSION);
    prefix.push(kind);
    prefix.extend_from_slice(&generation.to_le_bytes());
    prefix.extend_from_slice(&source);
    prefix.extend_from_slice(&options);
    prefix.extend_from_slice(&page.to_le_bytes());
    for len in lengths {
        prefix.extend_from_slice(&u32::try_from(len).map_err(|_| ContinuationError::PayloadSize)?.to_le_bytes());
    }
    debug_assert_eq!(prefix.len(), PREFIX);
    let mut body = Vec::with_capacity(body_len);
    body.extend_from_slice(prev);
    body.extend_from_slice(checkpoint);
    body.extend_from_slice(wire);
    let mut out = Vec::with_capacity(HEADER + body_len);
    out.extend_from_slice(&prefix);
    out.extend_from_slice(&digest(&prefix, &body));
    out.extend_from_slice(&body);
    Ok(out)
}

pub(super) fn decode(bytes: &[u8], expected_source: [u8; 32], expected_options: [u8; 32]) -> Result<Record, ContinuationError> {
    if bytes.len() < HEADER {
        return Err(ContinuationError::Malformed);
    }
    if bytes.len() > MAX_RECORD {
        return Err(ContinuationError::PayloadSize);
    }
    if &bytes[0..4] != MAGIC {
        return Err(ContinuationError::Malformed);
    }
    if bytes[4] != VERSION {
        return Err(ContinuationError::WrongVersion);
    }
    let variant = bytes[5];
    if variant != 1 && variant != 2 {
        return Err(ContinuationError::Malformed);
    }
    let lengths = [read_u32(bytes, 82) as usize, read_u32(bytes, 86) as usize, read_u32(bytes, 90) as usize];
    let body_len = lengths.into_iter().try_fold(0usize, |acc, n| acc.checked_add(n)).ok_or(ContinuationError::OutOfBounds)?;
    if HEADER.checked_add(body_len) != Some(bytes.len()) {
        return Err(ContinuationError::Malformed);
    }
    if digest(&bytes[..PREFIX], &bytes[HEADER..]).as_slice() != &bytes[PREFIX..HEADER] {
        return Err(ContinuationError::Integrity);
    }
    if &bytes[14..46] != expected_source.as_slice() {
        return Err(ContinuationError::WrongSource);
    }
    if &bytes[46..78] != expected_options.as_slice() {
        return Err(ContinuationError::WrongOptions);
    }
    if (variant == 1 && lengths[1] == 0) || (variant == 2 && (lengths[1] != 0 || lengths[2] == 0)) {
        return Err(ContinuationError::Malformed);
    }
    let mut revision_bytes = [0u8; 8];
    revision_bytes.copy_from_slice(&bytes[6..14]);
    let generation = u64::from_le_bytes(revision_bytes);
    let first_page = read_u32(bytes, 78) as usize;
    let a = HEADER;
    let b = a + lengths[0];
    let c = b + lengths[1];
    let predecessor = (lengths[0] > 0).then(|| bytes[a..b].to_vec());
    let step = if variant == 1 {
        PageDeltaStep::Continue { sealed_page_wire: (lengths[2] > 0).then(|| bytes[c..].to_vec()), checkpoint: bytes[b..c].to_vec(), first_page }
    } else {
        PageDeltaStep::Complete { tail_wire: bytes[c..].to_vec(), first_page }
    };
    Ok(Record { generation, predecessor, step })
}
