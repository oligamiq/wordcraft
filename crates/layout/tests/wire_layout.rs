#![cfg(feature = "wire-layout")]

use wordcraft_doc::Document;
use wordcraft_layout::{LayoutCache, LayoutOptions, layout, wire};

#[test]
fn public_font_layout_roundtrip_preserves_pages_and_navigation() {
    let mut doc = Document::from_text("A paragraph with ffi and 123.\nAnother paragraph.");
    doc.styles.default_chr.font = Some("Source Sans 3".to_string());
    let l = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
    assert!(!l.pages.is_empty());
    assert!(!l.index.is_empty());
    let bytes = wire::encode(&l).unwrap();
    let decoded = wire::decode(&bytes).unwrap();
    assert_eq!(decoded.index, l.index);
    assert_eq!(decoded.pages.len(), l.pages.len());
    for (a, b) in l.pages.iter().zip(&decoded.pages) {
        assert_eq!((a.w, a.h, a.number), (b.w, b.h, b.number));
        let x = wordcraft_layout::display::page_display(&doc, a, &Default::default());
        let y = wordcraft_layout::display::page_display(&doc, b, &Default::default());
        assert_eq!(format!("{x:?}"), format!("{y:?}"));
    }
    assert_eq!(wire::encode(&decoded).unwrap(), bytes);
}

#[test]
fn invalid_wire_inputs_fail_closed() {
    assert!(wire::decode(b"not a layout").is_err());
    assert!(wire::decode(b"FVLW\x01{").is_err());
    assert!(wire::decode(&vec![0; wire::MAX_WIRE_BYTES + 1]).is_err());
}

#[cfg(feature = "wire-layout-msgpack")]
#[test]
fn compact_wire_roundtrip_is_smaller_and_preserves_display() {
    let mut doc = Document::from_text(&"A repeated paragraph with ffi and 123.\n".repeat(32));
    doc.styles.default_chr.font = Some("Source Sans 3".to_string());
    let source = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
    let json = wire::encode(&source).unwrap();
    let compact = wire::encode_compact(&source).unwrap();
    assert!(compact.len() < json.len(), "{} >= {}", compact.len(), json.len());
    let decoded = wire::decode_compact(&compact).unwrap();
    assert_eq!(decoded.index, source.index);
    assert_eq!(wire::encode(&decoded).unwrap(), json);
    assert_eq!(wire::encode_compact(&decoded).unwrap(), compact);
    assert!(wire::decode_compact(&json).is_err());
    assert!(wire::decode_compact(&compact[..compact.len() - 1]).is_err());
    let mut trailing = compact.clone();
    trailing.push(0);
    assert!(wire::decode_compact(&trailing).is_err());
    assert!(wire::decode_compact(&vec![0; wire::MAX_WIRE_BYTES + 1]).is_err());
}

/// Diagnostic native benchmark: not a Worker CPU measurement.
/// Run in release mode with --ignored --nocapture, on public synthetic text only.
#[cfg(feature = "wire-layout-msgpack")]
#[test]
#[ignore]
fn compare_wire_codec_costs() {
    use std::{hint::black_box, time::Instant};

    for repeated in [true, false] {
        let text = (0..120)
            .map(|i| {
                if repeated {
                    "A paragraph with ffi and 123, and a second sentence.".to_string()
                } else {
                    format!("Paragraph {i}: {} {}", "office ".repeat(i % 7 + 1), i * 19)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut doc = Document::from_text(&text);
        doc.styles.default_chr.font = Some("Source Sans 3".to_string());
        let source = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
        assert!(source.pages.len() > 1);
        let reference = wire::encode(&source).unwrap();
        let compact = wire::encode_compact(&source).unwrap();
        assert!(compact.len() < reference.len());
        let mut samples: [Vec<f64>; 4] = Default::default();
        for i in 0..44 {
            for compact_mode in if i % 2 == 0 { [false, true] } else { [true, false] } {
                let t = Instant::now();
                let bytes = if compact_mode { wire::encode_compact(black_box(&source)).unwrap() } else { wire::encode(black_box(&source)).unwrap() };
                let encode_ms = t.elapsed().as_secs_f64() * 1000.0;
                let t = Instant::now();
                let decoded = if compact_mode { wire::decode_compact(black_box(&bytes)).unwrap() } else { wire::decode(black_box(&bytes)).unwrap() };
                let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(decoded.index, source.index);
                assert_eq!(wire::encode(&decoded).unwrap(), reference);
                assert_eq!(bytes, *if compact_mode { &compact } else { &reference });
                if i >= 12 {
                    let j = if compact_mode { 2 } else { 0 };
                    samples[j].push(encode_ms);
                    samples[j + 1].push(decode_ms);
                }
            }
        }
        println!("WIRE_CODEC_NATIVE repeated={repeated} pages={} json_bytes={} compact_bytes={}", source.pages.len(), reference.len(), compact.len());
        for (name, mut s) in ["json_encode", "json_decode", "compact_encode", "compact_decode"].into_iter().zip(samples) {
            assert_eq!(s.len(), 32);
            let mean = s.iter().sum::<f64>() / s.len() as f64;
            s.sort_by(f64::total_cmp);
            println!("WIRE_CODEC_NATIVE {name} n=32 mean_ms={mean:.4} p95_ms={:.4}", s[30]);
        }
    }
}
