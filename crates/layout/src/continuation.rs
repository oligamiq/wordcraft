//! Experimental source-bound transport envelope for future durable layout continuation.
//! This format transports bytes, not a resumed layout state. Only trusted internal callers
//! should provide the payload: SHA-256 detects corruption, not forgery or authenticity.

use sha2::{Digest, Sha256};

pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"FVLC";
const VERSION: u8 = 1;
const PREFIX_LEN: usize = 86;
const HEADER_LEN: usize = PREFIX_LEN + 32;
const MAX_ITERATION: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationError {
    Malformed,
    WrongVersion,
    InvalidPhase,
    WrongSource,
    WrongOptions,
    Integrity,
    OutOfBounds,
    PayloadSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Paginating = 1,
    Finalizing = 2,
}

impl Phase {
    fn decode(raw: u8) -> Result<Self, ContinuationError> {
        match raw {
            1 => Ok(Self::Paginating),
            2 => Ok(Self::Finalizing),
            _ => Err(ContinuationError::InvalidPhase),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub phase: Phase,
    pub iteration: u32,
    pub section: u32,
    pub block: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationEnvelope {
    pub source_digest: [u8; 32],
    pub options_digest: [u8; 32],
    pub cursor: Cursor,
    pub payload: Vec<u8>,
}

fn validate_cursor(cursor: Cursor) -> Result<(), ContinuationError> {
    if cursor.iteration > MAX_ITERATION {
        return Err(ContinuationError::OutOfBounds);
    }
    Ok(())
}

fn digest(prefix: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(payload);
    hasher.finalize().into()
}

fn read_u32(input: &[u8], start: usize) -> u32 {
    let mut chunk = [0; 4];
    chunk.copy_from_slice(&input[start..start + 4]);
    u32::from_le_bytes(chunk)
}

/// Packs an opaque continuation payload, deliberately without invoking layout.
///
/// Caller must supply digests of the complete immutable source and layout options.
pub fn encode(envelope: &ContinuationEnvelope) -> Result<Vec<u8>, ContinuationError> {
    validate_cursor(envelope.cursor)?;
    if envelope.payload.is_empty() || envelope.payload.len() > MAX_PAYLOAD {
        return Err(ContinuationError::PayloadSize);
    }
    let payload_len = u32::try_from(envelope.payload.len()).map_err(|_| ContinuationError::PayloadSize)?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + envelope.payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.push(envelope.cursor.phase as u8);
    bytes.extend_from_slice(&envelope.cursor.iteration.to_le_bytes());
    bytes.extend_from_slice(&envelope.cursor.section.to_le_bytes());
    bytes.extend_from_slice(&envelope.cursor.block.to_le_bytes());
    bytes.extend_from_slice(&envelope.source_digest);
    bytes.extend_from_slice(&envelope.options_digest);
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    let sum = digest(&bytes, &envelope.payload);
    bytes.extend_from_slice(&sum);
    bytes.extend_from_slice(&envelope.payload);
    Ok(bytes)
}

/// Verifies a portable envelope but does not rehydrate pagination state.
/// All inputs must be kept inside the authenticated gateway/private Worker boundary.
pub fn decode(
    bytes: &[u8],
    expected_source: [u8; 32],
    expected_options: [u8; 32],
    section_limit: u32,
    block_limit: u32,
) -> Result<ContinuationEnvelope, ContinuationError> {
    if bytes.len() < HEADER_LEN {
        return Err(ContinuationError::Malformed);
    }
    if &bytes[..4] != MAGIC {
        return Err(ContinuationError::Malformed);
    }
    if bytes[4] != VERSION {
        return Err(ContinuationError::WrongVersion);
    }
    let phase = Phase::decode(bytes[5])?;
    let cursor = Cursor { phase, iteration: read_u32(bytes, 6), section: read_u32(bytes, 10), block: read_u32(bytes, 14) };
    validate_cursor(cursor)?;
    let payload_len = usize::try_from(read_u32(bytes, 82)).map_err(|_| ContinuationError::PayloadSize)?;
    if payload_len == 0 || payload_len > MAX_PAYLOAD {
        return Err(ContinuationError::PayloadSize);
    }
    if bytes.len() != HEADER_LEN + payload_len {
        return Err(ContinuationError::Malformed);
    }
    if cursor.section > section_limit || cursor.block > block_limit {
        return Err(ContinuationError::OutOfBounds);
    }
    let payload = &bytes[HEADER_LEN..];
    if digest(&bytes[..PREFIX_LEN], payload).as_slice() != &bytes[PREFIX_LEN..HEADER_LEN] {
        return Err(ContinuationError::Integrity);
    }
    if &bytes[18..50] != expected_source.as_slice() {
        return Err(ContinuationError::WrongSource);
    }
    if &bytes[50..82] != expected_options.as_slice() {
        return Err(ContinuationError::WrongOptions);
    }
    Ok(ContinuationEnvelope { source_digest: expected_source, options_digest: expected_options, cursor, payload: payload.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ContinuationEnvelope {
        ContinuationEnvelope {
            source_digest: [1; 32],
            options_digest: [2; 32],
            cursor: Cursor { phase: Phase::Paginating, iteration: 2, section: 1, block: 4 },
            payload: vec![9, 8, 7],
        }
    }

    #[test]
    fn envelope_roundtrip() {
        let envelope = fixture();
        let bytes = encode(&envelope).unwrap();
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 3, 10).unwrap(), envelope);
    }

    #[test]
    fn wrong_source_and_options_are_rejected() {
        let bytes = encode(&fixture()).unwrap();
        assert_eq!(decode(&bytes, [3; 32], [2; 32], 3, 10), Err(ContinuationError::WrongSource));
        assert_eq!(decode(&bytes, [1; 32], [3; 32], 3, 10), Err(ContinuationError::WrongOptions));
    }

    #[test]
    fn corrupted_version_phase_and_payload_fail_closed() {
        let bytes = encode(&fixture()).unwrap();
        let mut version = bytes.clone();
        version[4] = 44;
        assert_eq!(decode(&version, [1; 32], [2; 32], 3, 10), Err(ContinuationError::WrongVersion));
        let mut phase = bytes.clone();
        phase[5] = 127;
        assert_eq!(decode(&phase, [1; 32], [2; 32], 3, 10), Err(ContinuationError::InvalidPhase));
        let mut altered = bytes.clone();
        *altered.last_mut().unwrap() ^= 1;
        assert_eq!(decode(&altered, [1; 32], [2; 32], 3, 10), Err(ContinuationError::Integrity));
    }

    #[test]
    fn truncated_extra_and_invalid_cursor_fail_closed() {
        let bytes = encode(&fixture()).unwrap();
        assert_eq!(decode(&bytes[..bytes.len() - 1], [1; 32], [2; 32], 3, 10), Err(ContinuationError::Malformed));
        let mut extended = bytes.clone();
        extended.push(0);
        assert_eq!(decode(&extended, [1; 32], [2; 32], 3, 10), Err(ContinuationError::Malformed));
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 0, 10), Err(ContinuationError::OutOfBounds));
        assert_eq!(decode(&bytes, [1; 32], [2; 32], 3, 3), Err(ContinuationError::OutOfBounds));
    }

    #[test]
    fn empty_or_oversized_payload_is_never_emitted() {
        let mut envelope = fixture();
        envelope.payload.clear();
        assert_eq!(encode(&envelope), Err(ContinuationError::PayloadSize));
        envelope.payload.resize(MAX_PAYLOAD + 1, 7);
        assert_eq!(encode(&envelope), Err(ContinuationError::PayloadSize));
    }

    #[test]
    fn invalid_encoding_cursor_is_never_emitted() {
        let mut envelope = fixture();
        envelope.cursor.iteration = u32::MAX;
        assert_eq!(encode(&envelope), Err(ContinuationError::OutOfBounds));
    }
}
// A complete multipass-layout boundary is the first true resumable execution unit.
// Block-level pause/restore, notes and early-float rewrap remain later stages.
#[cfg(test)]
mod pass_boundary_tests {
    use super::*;
    use crate::{LayoutCache, LayoutOptions, layout, wire};
    use wordcraft_doc::section::{NoteProps, NoteRestart};
    use wordcraft_doc::{Document, InlineObject, Path, StoryRef};

    fn totals_fixture() -> Document {
        let mut doc = Document::from_text(&"Text on a separate line.\n".repeat(130));
        let p = doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        let field =
            InlineObject::Field { instr: "NUMPAGES".into(), result: "9".into(), locked: false, dirty: false, code: false, form: String::new() };
        p.insert_object(p.len(), field, &Default::default()).unwrap();
        doc
    }

    #[test]
    fn page_total_restarts_only_the_next_pass_across_fresh_caches() {
        let doc = totals_fixture();
        let opts = LayoutOptions::default();
        let expected = layout(&doc, &mut LayoutCache::new(), &opts);
        assert!(expected.pages.len() > 1);
        let original = wire::encode(&expected).unwrap();
        let mut checkpoint = None;
        let mut took_continuation = false;
        let mut result = None;
        for _ in 0..8 {
            let mut cache = LayoutCache::new(); // model a fresh Worker / WASM instance
            match layout_one_pass(&doc, &mut cache, &opts, checkpoint.as_deref(), [0x11; 32], [0x22; 32]).unwrap() {
                LayoutPassStep::Continue(blob) => {
                    assert!(!blob.is_empty());
                    checkpoint = Some(blob);
                    took_continuation = true;
                }
                LayoutPassStep::Complete(layout) => {
                    result = Some(layout);
                    break;
                }
            }
        }
        assert!(took_continuation, "fixture must force at least two real passes");
        assert_eq!(wire::encode(&result.unwrap()).unwrap(), original);
    }

    #[test]
    fn restart_rejects_wrong_source_options_and_payload_phase() {
        let doc = totals_fixture();
        let opts = LayoutOptions::default();
        let mut cache = LayoutCache::new();
        let LayoutPassStep::Continue(bytes) = layout_one_pass(&doc, &mut cache, &opts, None, [0x11; 32], [0x22; 32]).unwrap() else {
            panic!("expected a totals correction pass")
        };
        assert!(matches!(layout_one_pass(&doc, &mut cache, &opts, Some(&bytes), [0x44; 32], [0x22; 32]), Err(ContinuationError::WrongSource)));
        assert!(matches!(layout_one_pass(&doc, &mut cache, &opts, Some(&bytes), [0x11; 32], [0x44; 32]), Err(ContinuationError::WrongOptions)));
        let mut bad = decode(&bytes, [0x11; 32], [0x22; 32], u32::MAX, u32::MAX).unwrap();
        bad.cursor.phase = Phase::Finalizing;
        let forged = encode(&bad).unwrap();
        assert!(matches!(layout_one_pass(&doc, &mut cache, &opts, Some(&forged), [0x11; 32], [0x22; 32]), Err(ContinuationError::InvalidState)));
    }

    #[test]
    fn unsupported_page_footnote_restart_fails_without_partial_output() {
        let mut doc = totals_fixture();
        doc.settings.footnote_pr = NoteProps { num_restart: Some(NoteRestart::EachPage), ..Default::default() };
        assert!(matches!(
            layout_one_pass(&doc, &mut LayoutCache::new(), &LayoutOptions::default(), None, [0x11; 32], [0x22; 32]),
            Err(ContinuationError::Unsupported)
        ));
    }
}
