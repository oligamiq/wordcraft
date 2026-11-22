//! Experimental source-bound transport envelope for future durable layout continuation.
//! This format transports bytes, not a resumed layout state. Only trusted internal callers
//! should provide the payload: SHA-256 detects corruption, not forgery or authenticity.

use sha2::{Digest, Sha256};

mod collector;
mod journal;
pub use collector::{CollectorAcceptance, PageDeltaCollector};

#[cfg(test)]
mod journal_tests;

// One codec for all private continuation checkpoints: MessagePack, with no
// No payload fallback to other encodings. The FVLC envelope is unchanged.
const CHECKPOINT_MARKER: &[u8; 4] = b"MPK1";

fn checkpoint_to_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ContinuationError> {
    let mut bytes = CHECKPOINT_MARKER.to_vec();
    value
        .serialize(&mut rmp_serde::Serializer::new(&mut bytes).with_struct_map().with_human_readable())
        .map_err(|_| ContinuationError::InvalidState)?;
    Ok(bytes)
}

fn checkpoint_from_bytes<T: serde::de::DeserializeOwned>(input: &[u8]) -> Result<T, ContinuationError> {
    let data = input.strip_prefix(CHECKPOINT_MARKER).ok_or(ContinuationError::InvalidState)?;
    let mut decoder = rmp_serde::Deserializer::new(std::io::Cursor::new(data)).with_human_readable();
    let value = T::deserialize(&mut decoder).map_err(|_| ContinuationError::InvalidState)?;
    if decoder.get_ref().position() != data.len() as u64 {
        return Err(ContinuationError::InvalidState);
    }
    Ok(value)
}

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
    InvalidState,
    Unsupported,
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

/// One complete layout pass per invocation; this is not block-level continuation.
/// Each Continue checkpoint contains only total-page pass state. Multipass
/// pagination may necessarily reflow earlier pages in subsequent passes.
pub enum LayoutPassStep {
    Continue(Vec<u8>),
    Complete(crate::DocLayout),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PassCheckpoint {
    totals: crate::cache::Totals,
    passes: u32,
}

/// Advances just one complete multipass layout pass using a portable FVLC envelope.
///
/// This experimental API deliberately declines inputs requiring per-page note restarts
/// or early-float rewrap; those need more state. It does not limit CPU inside one pass.
/// Source and options digests must be calculated and bound by the trusted caller
/// from canonical immutable input; this API does not authenticate them.
pub fn layout_one_pass(
    doc: &crate::Document,
    cache: &mut crate::LayoutCache,
    opts: &crate::LayoutOptions,
    checkpoint: Option<&[u8]>,
    source_digest: [u8; 32],
    options_digest: [u8; 32],
) -> Result<LayoutPassStep, ContinuationError> {
    use crate::cache::MAX_TOTAL_PASSES;
    use crate::ctx::Pass;
    use crate::floats::EarlyFloats;
    use crate::{HashMap, ViewMode};
    use wordcraft_doc::section::NoteRestart;

    let sections = doc.sections();
    let per_page = opts.view == ViewMode::Print && sections.iter().any(|(_, s)| doc.note_options(s, false).num_restart == NoteRestart::EachPage);
    if per_page {
        return Err(ContinuationError::Unsupported);
    }

    let state = match checkpoint {
        None => PassCheckpoint { totals: crate::cache::Totals::first_guess(sections.len()), passes: 0 },
        Some(bytes) => {
            let section_limit = u32::try_from(sections.len()).map_err(|_| ContinuationError::OutOfBounds)?;
            let block_limit = u32::try_from(doc.body.len()).map_err(|_| ContinuationError::OutOfBounds)?;
            let envelope = decode(bytes, source_digest, options_digest, section_limit, block_limit)?;
            if envelope.cursor.phase != Phase::Paginating || envelope.cursor.section != 0 || envelope.cursor.block != 0 {
                return Err(ContinuationError::InvalidState);
            }
            let state: PassCheckpoint = checkpoint_from_bytes(&envelope.payload)?;
            if state.passes != envelope.cursor.iteration || state.passes > MAX_TOTAL_PASSES as u32 || !state.totals.valid_checkpoint(sections.len()) {
                return Err(ContinuationError::InvalidState);
            }
            state
        }
    };

    // A new instance can reconstruct immutable section/header references from doc.
    // No raw pointers or layout caches are carried across this pass boundary.
    let empty_note_pages = HashMap::new();
    let empty_early_floats = EarlyFloats::new();
    let outcome = crate::layout_pass(
        doc,
        cache,
        opts,
        &empty_note_pages,
        &state.totals,
        state.passes < MAX_TOTAL_PASSES as u32,
        (&empty_early_floats, true),
        usize::MAX,
    );
    match outcome {
        Pass::Again(totals) => {
            let next_pass = state.passes.checked_add(1).ok_or(ContinuationError::InvalidState)?;
            if next_pass > MAX_TOTAL_PASSES as u32 || !totals.valid_checkpoint(sections.len()) {
                return Err(ContinuationError::InvalidState);
            }
            let next = PassCheckpoint { totals, passes: next_pass };
            let payload = checkpoint_to_bytes(&next)?;
            let envelope = ContinuationEnvelope {
                source_digest,
                options_digest,
                cursor: Cursor { phase: Phase::Paginating, iteration: next_pass, section: 0, block: 0 },
                payload,
            };
            Ok(LayoutPassStep::Continue(encode(&envelope)?))
        }
        Pass::Done(layout) => Ok(LayoutPassStep::Complete(layout)),
        Pass::Rewrap(_) => Err(ContinuationError::Unsupported),
    }
}

/// A portable in-pass checkpoint for plain, single-section, one-column print
/// documents only. It owns completed page items and open-page position. The
/// caller must compute and bind the full source/options SHA-256 digests.
#[derive(serde::Serialize, serde::Deserialize)]
struct BlockCheckpoint {
    schema: u32,
    state: crate::pass::PlainBlockSnapshot,
}

/// Advances at most max_blocks body blocks inside one layout pass.
///
/// No prior body paragraph is relaid on resume. Complex documents return
/// Unsupported until notes/floats/column balancing have portable snapshots.
/// An invocation can still exceed 10ms due to one block or packet copying.
pub fn layout_in_resumable_blocks(
    doc: &crate::Document,
    cache: &mut crate::LayoutCache,
    opts: &crate::LayoutOptions,
    checkpoint: Option<&[u8]>,
    source_digest: [u8; 32],
    options_digest: [u8; 32],
    max_blocks: usize,
) -> Result<LayoutPassStep, ContinuationError> {
    if max_blocks == 0 {
        return Err(ContinuationError::OutOfBounds);
    }
    if !crate::pass::plain_supported(doc, opts) {
        return Err(ContinuationError::Unsupported);
    }
    let saved = match checkpoint {
        None => None,
        Some(bytes) => {
            let len = u32::try_from(doc.body.len()).map_err(|_| ContinuationError::OutOfBounds)?;
            let envelope = decode(bytes, source_digest, options_digest, 1, len)?;
            if envelope.cursor.phase != Phase::Paginating || envelope.cursor.iteration != 0 || envelope.cursor.section != 0 {
                return Err(ContinuationError::InvalidState);
            }
            let decoded: BlockCheckpoint = checkpoint_from_bytes(&envelope.payload)?;
            if decoded.schema != 1
                || decoded.state.next_block == 0
                || decoded.state.next_block >= doc.body.len()
                || decoded.state.next_block != envelope.cursor.block as usize
            {
                return Err(ContinuationError::InvalidState);
            }
            Some(decoded.state)
        }
    };

    match crate::pass::step_plain(doc, cache, opts, saved, max_blocks)? {
        crate::pass::PlainStep::Complete(layout) => Ok(LayoutPassStep::Complete(layout)),
        crate::pass::PlainStep::Continue(state) => {
            let block = u32::try_from(state.next_block).map_err(|_| ContinuationError::OutOfBounds)?;
            let checkpoint = BlockCheckpoint { schema: 1, state };
            let payload = checkpoint_to_bytes(&checkpoint)?;
            let envelope = ContinuationEnvelope {
                source_digest,
                options_digest,
                cursor: Cursor { phase: Phase::Paginating, iteration: 0, section: 0, block },
                payload,
            };
            Ok(LayoutPassStep::Continue(encode(&envelope)?))
        }
    }
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
    fn checkpoint_uses_one_messagepack_format_and_rejects_json() {
        let input = PassCheckpoint { totals: crate::cache::Totals::first_guess(2), passes: 3 };
        let encoded = checkpoint_to_bytes(&input).expect("encode MessagePack checkpoint");
        assert!(encoded.starts_with(CHECKPOINT_MARKER));
        let restored: PassCheckpoint = checkpoint_from_bytes(&encoded).expect("decode MessagePack checkpoint");
        assert_eq!(restored.passes, input.passes);
        assert_eq!(restored.totals, input.totals);

        let legacy = serde_json::to_vec(&input).unwrap();
        assert!(checkpoint_from_bytes::<PassCheckpoint>(&legacy).is_err(), "must not mix checkpoint codecs");
        let mut with_trailing = encoded;
        with_trailing.push(0);
        assert!(checkpoint_from_bytes::<PassCheckpoint>(&with_trailing).is_err(), "must reject trailing bytes");
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
        doc.styles.default_chr.font = Some("Source Sans 3".into());
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

// Stage 2b: a true in-pass cursor, beginning with the strictly plain body subset.
// Complex notes/tables/floats/columns remain unsupported until their state is owned.
#[cfg(test)]
mod block_boundary_tests {
    use super::*;
    use crate::{LayoutCache, LayoutOptions, layout, wire};
    use wordcraft_doc::{Document, InlineObject, Path, StoryRef};

    fn plain_doc() -> Document {
        let mut doc = Document::from_text(&"A sentence with enough words to take some room.\n".repeat(140));
        doc.styles.default_chr.font = Some("Source Sans 3".into());
        doc
    }

    #[test]
    fn resume_body_blocks_across_new_instances_without_prefix_relayout() {
        let doc = plain_doc();
        let opts = LayoutOptions::default();
        let reference = layout(&doc, &mut LayoutCache::new(), &opts);
        let expected = wire::encode(&reference).unwrap();
        let expected_interned = wire::encode_interned(&reference).unwrap();
        assert!(doc.body.len() >= 100);
        for batch_size in [1usize, 3, 13, 41] {
            crate::pass::reset_placed_visits();
            let mut continuation: Option<Vec<u8>> = None;
            let mut progressed = 0usize;
            let mut completed = None;
            for _ in 0..=doc.body.len() {
                let mut cache = LayoutCache::new();
                let input = continuation.as_deref();
                match layout_in_resumable_blocks(&doc, &mut cache, &opts, input, [0x31; 32], [0x42; 32], batch_size).unwrap() {
                    LayoutPassStep::Continue(bytes) => {
                        let envelope = decode(&bytes, [0x31; 32], [0x42; 32], 1, doc.body.len() as u32).unwrap();
                        let next = envelope.cursor.block as usize;
                        assert_eq!(next, (progressed + batch_size).min(doc.body.len()));
                        assert!(next > progressed);
                        progressed = next;
                        continuation = Some(bytes);
                    }
                    LayoutPassStep::Complete(output) => {
                        completed = Some(output);
                        break;
                    }
                }
            }
            assert_eq!(progressed + batch_size >= doc.body.len(), true, "must reach final blocks");
            assert_eq!(crate::pass::take_placed_visits(), (0..doc.body.len()).collect::<Vec<_>>(), "each top-level block exactly once");
            let actual = completed.expect("completed");
            assert_eq!(actual.index, reference.index, "restored index differs");
            assert_eq!(wire::encode(&actual).unwrap(), expected, "batch_size={batch_size}");
            assert_eq!(wire::encode_interned(&actual).unwrap(), expected_interned, "v3 batch_size={batch_size}");
        }
    }

    #[test]
    fn rejects_unsupported_structures_and_mismatched_checkpoints() {
        let doc = plain_doc();
        let opts = LayoutOptions::default();
        assert!(matches!(
            layout_in_resumable_blocks(&doc, &mut LayoutCache::new(), &opts, None, [3; 32], [4; 32], 0),
            Err(ContinuationError::OutOfBounds)
        ));

        let first = layout_in_resumable_blocks(&doc, &mut LayoutCache::new(), &opts, None, [3; 32], [4; 32], 3).unwrap();
        let LayoutPassStep::Continue(bytes) = first else { panic!("expected a checkpoint") };
        assert!(matches!(
            layout_in_resumable_blocks(&doc, &mut LayoutCache::new(), &opts, Some(&bytes), [5; 32], [4; 32], 3),
            Err(ContinuationError::WrongSource)
        ));
        assert!(matches!(
            layout_in_resumable_blocks(&doc, &mut LayoutCache::new(), &opts, Some(&bytes), [3; 32], [5; 32], 3),
            Err(ContinuationError::WrongOptions)
        ));

        let mut wrong = decode(&bytes, [3; 32], [4; 32], 1, doc.body.len() as u32).unwrap();
        wrong.cursor.block += 1;
        let wrong = encode(&wrong).unwrap();
        assert!(matches!(
            layout_in_resumable_blocks(&doc, &mut LayoutCache::new(), &opts, Some(&wrong), [3; 32], [4; 32], 3),
            Err(ContinuationError::InvalidState)
        ));

        let mut field_doc = plain_doc();
        let p = field_doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        p.insert_object(
            p.len(),
            InlineObject::Field { instr: "PAGE".into(), result: "1".into(), locked: false, dirty: false, code: false, form: String::new() },
            &Default::default(),
        )
        .unwrap();
        assert!(matches!(
            layout_in_resumable_blocks(&field_doc, &mut LayoutCache::new(), &opts, None, [3; 32], [4; 32], 3),
            Err(ContinuationError::Unsupported)
        ));

        let mut columns_doc = plain_doc();
        columns_doc.last_section.columns.count = 2;
        assert!(matches!(
            layout_in_resumable_blocks(&columns_doc, &mut LayoutCache::new(), &opts, None, [3; 32], [4; 32], 3),
            Err(ContinuationError::Unsupported)
        ));
    }
}

// Stage 2b-2. Sealed pages move exactly once; the FVLC cursor owns only the open page.
#[cfg(test)]
mod page_delta_tests {
    use super::*;
    use crate::{DocLayout, LayoutCache, LayoutOptions, layout, pass, wire};
    use wordcraft_doc::Document;

    fn fixture() -> Document {
        let mut doc = Document::from_text(&"An ordinary paragraph taking a page line.\n".repeat(220));
        doc.styles.default_chr.font = Some("Source Sans 3".into());
        doc
    }

    #[test]
    fn streamed_sealed_pages_match_whole_layout_and_checkpoint_has_only_open_page() {
        let doc = fixture();
        let opts = LayoutOptions::default();
        let reference = layout(&doc, &mut LayoutCache::new(), &opts);
        let expected_v1 = wire::encode(&reference).unwrap();
        let expected_v3 = wire::encode_interned(&reference).unwrap();
        assert!(reference.pages.len() >= 3, "fixture must span pages");
        for batch_size in [1, 5, 17, 73] {
            pass::reset_placed_visits();
            let mut checkpoint: Option<Vec<u8>> = None;
            let mut pages = Vec::new();
            let mut complete = false;
            for _ in 0..=doc.body.len() {
                let output =
                    layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, checkpoint.as_deref(), [31; 32], [47; 32], batch_size).unwrap();
                match output {
                    PageDeltaStep::Continue { sealed_page_wire, checkpoint: next, first_page } => {
                        assert_eq!(first_page, pages.len());
                        if let Some(packet) = sealed_page_wire {
                            let chunk = wire::decode(&packet).unwrap();
                            assert!(!chunk.pages.is_empty());
                            pages.extend(chunk.pages);
                        }
                        let env = decode(&next, [31; 32], [47; 32], 1, doc.body.len() as u32).unwrap();
                        assert!(env.payload.starts_with(CHECKPOINT_MARKER));
                        let data: BlockDeltaCheckpoint = checkpoint_from_bytes(&env.payload).unwrap();
                        assert_eq!(data.schema, 2);
                        assert_eq!(data.state.pages.len(), 1);
                        assert_eq!(data.state.behind.len(), 1);
                        assert_eq!(data.state.page_base, pages.len());
                        checkpoint = Some(next);
                    }
                    PageDeltaStep::Complete { tail_wire, first_page } => {
                        assert_eq!(first_page, pages.len());
                        pages.extend(wire::decode(&tail_wire).unwrap().pages);
                        complete = true;
                        break;
                    }
                }
            }
            assert!(complete);
            assert_eq!(pass::take_placed_visits(), (0..doc.body.len()).collect::<Vec<_>>());
            let result = DocLayout { index: pass::index_lines(&pages), pages, ..Default::default() };
            assert_eq!(result.index, reference.index);
            assert_eq!(wire::encode(&result).unwrap(), expected_v1);
            assert_eq!(wire::encode_interned(&result).unwrap(), expected_v3);
        }
    }

    #[test]
    fn page_delta_rejects_wrong_digest_and_forged_cursor() {
        let doc = fixture();
        let opts = LayoutOptions::default();
        let first = layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, None, [31; 32], [47; 32], 1).unwrap();
        let PageDeltaStep::Continue { checkpoint, .. } = first else { panic!("expected continuation") };
        assert!(matches!(
            layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, Some(&checkpoint), [32; 32], [47; 32], 1),
            Err(ContinuationError::WrongSource)
        ));
        let mut envelope = decode(&checkpoint, [31; 32], [47; 32], 1, doc.body.len() as u32).unwrap();
        envelope.cursor.block += 1;
        let forged = encode(&envelope).unwrap();
        assert!(matches!(
            layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, Some(&forged), [31; 32], [47; 32], 1),
            Err(ContinuationError::InvalidState)
        ));
    }
}

/// An incremental page packet at a complete body-block boundary.
/// Both packet forms use the canonical FVLW v1 page representation.
/// Assemble these typed pages in order before FVLW v3 or PDF finalization.
pub enum PageDeltaStep {
    Continue { sealed_page_wire: Option<Vec<u8>>, checkpoint: Vec<u8>, first_page: usize },
    Complete { tail_wire: Vec<u8>, first_page: usize },
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BlockDeltaCheckpoint {
    schema: u32,
    state: crate::pass::PlainBlockSnapshot,
}

/// Advances the plain-body subset without resending already sealed page bodies.
/// A prior page is immutable after a later page opens in this restricted mode;
/// only the active page is present in the FVLC checkpoint. The caller persists
/// emitted FVLW page packets and assembles a complete layout before final PDF.
/// No CPU-limit guarantee or support for columns, notes, floats or totals yet.
pub fn layout_in_page_deltas(
    doc: &crate::Document,
    cache: &mut crate::LayoutCache,
    opts: &crate::LayoutOptions,
    checkpoint: Option<&[u8]>,
    source_digest: [u8; 32],
    options_digest: [u8; 32],
    max_blocks: usize,
) -> Result<PageDeltaStep, ContinuationError> {
    if max_blocks == 0 {
        return Err(ContinuationError::OutOfBounds);
    }
    if !crate::pass::plain_supported(doc, opts) {
        return Err(ContinuationError::Unsupported);
    }
    let saved = match checkpoint {
        None => None,
        Some(bytes) => {
            let len = u32::try_from(doc.body.len()).map_err(|_| ContinuationError::OutOfBounds)?;
            let env = decode(bytes, source_digest, options_digest, 1, len)?;
            if env.cursor.phase != Phase::Paginating || env.cursor.iteration != 0 || env.cursor.section != 0 {
                return Err(ContinuationError::InvalidState);
            }
            let stored: BlockDeltaCheckpoint = checkpoint_from_bytes(&env.payload)?;
            if stored.schema != 2
                || stored.state.next_block == 0
                || stored.state.next_block >= doc.body.len()
                || stored.state.next_block != env.cursor.block as usize
            {
                return Err(ContinuationError::InvalidState);
            }
            Some(stored.state)
        }
    };

    let packet =
        |pages: Vec<crate::Page>| crate::wire::encode(&crate::DocLayout { pages, ..Default::default() }).map_err(|_| ContinuationError::PayloadSize);
    match crate::pass::step_plain_delta(doc, cache, opts, saved, max_blocks)? {
        crate::pass::PlainDeltaStep::Continue { sealed, state, first_page } => {
            let sealed_page_wire = if sealed.is_empty() { None } else { Some(packet(sealed)?) };
            let cursor = Cursor {
                phase: Phase::Paginating,
                iteration: 0,
                section: 0,
                block: u32::try_from(state.next_block).map_err(|_| ContinuationError::OutOfBounds)?,
            };
            let payload = checkpoint_to_bytes(&BlockDeltaCheckpoint { schema: 2, state })?;
            let checkpoint = encode(&ContinuationEnvelope { source_digest, options_digest, cursor, payload })?;
            Ok(PageDeltaStep::Continue { sealed_page_wire, checkpoint, first_page })
        }
        crate::pass::PlainDeltaStep::Complete { pages, first_page } => Ok(PageDeltaStep::Complete { tail_wire: packet(pages)?, first_page }),
    }
}

// Stage 2b-3: reject stale/out-of-order page deliveries and accept exact retries.
#[cfg(test)]
mod collector_tests {
    use super::*;
    use crate::{LayoutCache, LayoutOptions, layout, wire};
    use wordcraft_doc::Document;

    fn source() -> Document {
        let mut doc = Document::from_text(&"Long enough to create multiple page breaks.\n".repeat(210));
        doc.styles.default_chr.font = Some("Source Sans 3".into());
        doc
    }

    fn responses(doc: &Document, opts: &LayoutOptions) -> Vec<(Option<Vec<u8>>, PageDeltaStep)> {
        let mut next = None;
        let mut result = Vec::new();
        loop {
            let response = layout_in_page_deltas(doc, &mut LayoutCache::new(), opts, next.as_deref(), [31; 32], [47; 32], 19).unwrap();
            let done = matches!(response, PageDeltaStep::Complete { .. });
            let incoming = next.clone();
            next = match &response {
                PageDeltaStep::Continue { checkpoint, .. } => Some(checkpoint.clone()),
                PageDeltaStep::Complete { .. } => None,
            };
            result.push((incoming, response));
            if done {
                break;
            }
        }
        result
    }

    #[test]
    fn duplicate_and_out_of_order_delivery_are_safe_and_final_layout_is_exact() {
        let doc = source();
        let opts = LayoutOptions::default();
        let reference = layout(&doc, &mut LayoutCache::new(), &opts);
        let events = responses(&doc, &opts);
        assert!(events.len() > 4);
        let mut collector = PageDeltaCollector::new([31; 32], [47; 32], doc.body.len() as u32);

        assert_eq!(collector.apply(events[0].0.as_deref(), &events[0].1), Ok(CollectorAcceptance::Applied));
        assert_eq!(collector.apply(events[0].0.as_deref(), &events[0].1), Ok(CollectorAcceptance::Duplicate));
        assert_eq!(collector.apply(events[2].0.as_deref(), &events[2].1), Err(ContinuationError::InvalidState));
        assert_eq!(collector.apply(events[1].0.as_deref(), &events[1].1), Ok(CollectorAcceptance::Applied));
        assert_eq!(collector.apply(events[0].0.as_deref(), &events[0].1), Ok(CollectorAcceptance::Duplicate));

        for (before, step) in &events[2..] {
            assert_eq!(collector.apply(before.as_deref(), step), Ok(CollectorAcceptance::Applied));
        }
        assert!(collector.is_complete());
        for (before, step) in events.iter().rev() {
            assert_eq!(collector.apply(before.as_deref(), step), Ok(CollectorAcceptance::Duplicate));
        }
        let final_layout = collector.into_layout().unwrap();
        assert_eq!(final_layout.index, reference.index);
        assert_eq!(wire::encode(&final_layout).unwrap(), wire::encode(&reference).unwrap());
        assert_eq!(wire::encode_interned(&final_layout).unwrap(), wire::encode_interned(&reference).unwrap());
    }

    #[test]
    fn rejects_conflicting_retries_and_bad_packet_without_mutating_collector() {
        let doc = source();
        let opts = LayoutOptions::default();
        let events = responses(&doc, &opts);
        let mut collector = PageDeltaCollector::new([31; 32], [47; 32], doc.body.len() as u32);
        let (first_before, first) = &events[0];
        assert_eq!(collector.apply(first_before.as_deref(), first), Ok(CollectorAcceptance::Applied));
        let original_count = collector.page_count();

        if let PageDeltaStep::Continue { sealed_page_wire, checkpoint, first_page } = first {
            let mut bad = checkpoint.clone();
            *bad.last_mut().unwrap() ^= 0x8;
            let corrupt = PageDeltaStep::Continue { sealed_page_wire: sealed_page_wire.clone(), checkpoint: bad, first_page: *first_page };
            assert_eq!(collector.apply(first_before.as_deref(), &corrupt), Err(ContinuationError::InvalidState));
            assert_eq!(collector.page_count(), original_count);
        } else {
            panic!("first step must be partial")
        }

        for (before, step) in events.iter().skip(1) {
            assert_eq!(collector.apply(before.as_deref(), step), Ok(CollectorAcceptance::Applied));
        }
        assert!(collector.is_complete());
        let count = collector.page_count();
        let fake = PageDeltaStep::Complete { tail_wire: wire::encode(&crate::DocLayout::default()).unwrap(), first_page: count };
        assert_eq!(collector.apply(events.last().unwrap().0.as_deref(), &fake), Err(ContinuationError::InvalidState));
        assert_eq!(collector.page_count(), count);
    }
}
