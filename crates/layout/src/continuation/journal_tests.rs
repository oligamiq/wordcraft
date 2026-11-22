//! Portable journal contracts, exercised with a fresh collector at each event.
use super::*;
use crate::{LayoutCache, LayoutOptions, layout, wire};
use wordcraft_doc::Document;

fn source() -> Document {
    let mut d = Document::from_text(&"Page-delivery log and deterministic output.\n".repeat(145));
    d.styles.default_chr.font = Some("Source Sans 3".into());
    d
}

#[test]
fn append_only_journal_recovers_after_every_coordinator_restart() {
    let doc = source();
    let opts = LayoutOptions::default();
    let original = layout(&doc, &mut LayoutCache::new(), &opts);
    let mut journal = Vec::<Vec<u8>>::new();
    let mut predecessor: Option<Vec<u8>> = None;
    let mut complete = false;
    for _ in 0..=doc.body.len() {
        // The producer and coordinator are recreated on every transition.
        let response = layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, predecessor.as_deref(), [31; 32], [47; 32], 17).unwrap();
        let collector = PageDeltaCollector::replay_journal([31; 32], [47; 32], doc.body.len() as u32, &journal).unwrap();
        assert_eq!(collector.generation(), journal.len() as u64);
        let record = collector.encode_journal_record(predecessor.as_deref(), &response).unwrap();
        let mut fresh = PageDeltaCollector::replay_journal([31; 32], [47; 32], doc.body.len() as u32, &journal).unwrap();
        assert_eq!(fresh.apply_journal_record(&record), Ok(CollectorAcceptance::Applied));
        assert_eq!(fresh.apply_journal_record(&record), Ok(CollectorAcceptance::Duplicate));
        assert_eq!(fresh.generation(), journal.len() as u64 + 1);
        complete = matches!(response, PageDeltaStep::Complete { .. });
        predecessor = match response {
            PageDeltaStep::Continue { checkpoint, .. } => Some(checkpoint),
            PageDeltaStep::Complete { .. } => None,
        };
        journal.push(record);
        if complete {
            break;
        }
    }
    assert!(complete, "finish with a terminal record");
    let mut restored = PageDeltaCollector::replay_journal([31; 32], [47; 32], doc.body.len() as u32, &journal).unwrap();
    assert!(restored.is_complete());
    for old in &journal {
        assert_eq!(restored.apply_journal_record(old), Ok(CollectorAcceptance::Duplicate));
    }
    let result = restored.into_layout().unwrap();
    assert_eq!(result.index, original.index);
    assert_eq!(wire::encode(&result).unwrap(), wire::encode(&original).unwrap());
    assert_eq!(wire::encode_interned(&result).unwrap(), wire::encode_interned(&original).unwrap());
}

#[test]
fn malformed_wrong_source_and_stale_generation_are_rejected_without_mutation() {
    let doc = source();
    let opts = LayoutOptions::default();
    let first = layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, None, [31; 32], [47; 32], 13).unwrap();
    let initial = PageDeltaCollector::new([31; 32], [47; 32], doc.body.len() as u32);
    let record = initial.encode_journal_record(None, &first).unwrap();
    let mut restored = PageDeltaCollector::new([31; 32], [47; 32], doc.body.len() as u32);

    let mut damaged = record.clone();
    *damaged.last_mut().unwrap() ^= 0x01;
    assert_eq!(restored.apply_journal_record(&damaged), Err(ContinuationError::Integrity));
    assert_eq!(restored.apply_journal_record(&record[..record.len() - 1]), Err(ContinuationError::Malformed));
    assert_eq!(restored.generation(), 0);
    assert_eq!(restored.page_count(), 0);

    let mut wrong_source = PageDeltaCollector::new([32; 32], [47; 32], doc.body.len() as u32);
    assert_eq!(wrong_source.apply_journal_record(&record), Err(ContinuationError::WrongSource));
    assert_eq!(wrong_source.page_count(), 0);

    let PageDeltaStep::Continue { checkpoint, .. } = &first else { panic!("must be partial") };
    let second = layout_in_page_deltas(&doc, &mut LayoutCache::new(), &opts, Some(checkpoint), [31; 32], [47; 32], 13).unwrap();
    // A stale writer prepared its own next generation before commit 0 was recorded.
    let stale_record = initial.encode_journal_record(Some(checkpoint), &second).unwrap();
    assert_eq!(restored.apply_journal_record(&record), Ok(CollectorAcceptance::Applied));
    assert_eq!(restored.generation(), 1);
    assert_eq!(restored.apply_journal_record(&stale_record), Err(ContinuationError::InvalidState));
    assert_eq!(restored.generation(), 1);
    assert_eq!(restored.apply_journal_record(&record), Ok(CollectorAcceptance::Duplicate));

    let prepared = restored.encode_journal_record(Some(checkpoint), &second).unwrap();
    assert_eq!(restored.apply_journal_record(&prepared), Ok(CollectorAcceptance::Applied));
    assert_eq!(restored.generation(), 2);
    let log = vec![record, prepared];
    let recovered = PageDeltaCollector::replay_journal([31; 32], [47; 32], doc.body.len() as u32, &log).unwrap();
    assert_eq!(recovered.generation(), 2);
    assert_eq!(recovered.checkpoint(), restored.checkpoint());
}
