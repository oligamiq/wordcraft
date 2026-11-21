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
        #[cfg(feature = "wire-layout-interned")]
        let interned = wire::encode_interned(&source).unwrap();
        #[cfg(feature = "wire-layout-interned")]
        assert!(interned.len() < compact.len());
        let mut samples: [Vec<f64>; 4] = Default::default();
        #[cfg(feature = "wire-layout-interned")]
        let mut interned_samples: [Vec<f64>; 2] = Default::default();
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
            #[cfg(feature = "wire-layout-interned")]
            {
                let t = Instant::now();
                let bytes = wire::encode_interned(black_box(&source)).unwrap();
                let encode_ms = t.elapsed().as_secs_f64() * 1000.0;
                let t = Instant::now();
                let decoded = wire::decode_interned(black_box(&bytes)).unwrap();
                let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(decoded.index, source.index);
                assert_eq!(wire::encode(&decoded).unwrap(), reference);
                assert_eq!(bytes, interned);
                if i >= 12 {
                    interned_samples[0].push(encode_ms);
                    interned_samples[1].push(decode_ms);
                }
            }
        }
        #[cfg(feature = "wire-layout-interned")]
        {
            println!("WIRE_CODEC_NATIVE repeated={repeated} interned_bytes={}", interned.len());
            for (name, mut s) in ["interned_encode", "interned_decode"].into_iter().zip(interned_samples) {
                assert_eq!(s.len(), 32);
                let mean = s.iter().sum::<f64>() / s.len() as f64;
                s.sort_by(f64::total_cmp);
                println!("WIRE_CODEC_NATIVE {name} n=32 mean_ms={mean:.4} p95_ms={:.4}", s[30]);
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

#[cfg(feature = "wire-layout-interned")]
#[test]
fn interned_layout_reuses_repeated_data_without_changing_layout() {
    let mut doc = Document::from_text(&"A repeated paragraph with ffi and 123.\n".repeat(96));
    doc.styles.default_chr.font = Some("Source Sans 3".to_string());
    let source = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
    let json = wire::encode(&source).unwrap();
    let compact = wire::encode_compact(&source).unwrap();
    let interned = wire::encode_interned(&source).unwrap();
    assert!(interned.len() * 2 < compact.len(), "{} >= half of {}", interned.len(), compact.len());
    let decoded = wire::decode_interned(&interned).unwrap();
    assert_eq!(decoded.index, source.index);
    assert_eq!(wire::encode(&decoded).unwrap(), json);
    assert_eq!(wire::encode_interned(&decoded).unwrap(), interned);
    assert!(wire::decode_interned(&compact).is_err());
    assert!(wire::decode_interned(&interned[..interned.len() - 1]).is_err());
    let mut trailing = interned.clone();
    trailing.push(0);
    assert!(wire::decode_interned(&trailing).is_err());
}

#[cfg(feature = "wire-layout-interned")]
#[test]
fn interned_encoder_page_batches_preserve_exact_wire_bytes() {
    let mut doc = Document::from_text(&"A repeated paragraph with office ffi and 123.\n".repeat(240));
    doc.styles.default_chr.font = Some("Source Sans 3".to_string());
    let source = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
    assert!(source.pages.len() >= 3);
    let canonical = wire::encode_interned(&source).unwrap();

    for batch in [1, 2, 3, 7, usize::MAX] {
        let mut encoder = wire::InternedEncoder::new(&source);
        assert_eq!(encoder.remaining_pages(), source.pages.len());
        assert!(encoder.push_pages(0).is_err());
        while encoder.remaining_pages() > 0 {
            let remaining = encoder.remaining_pages();
            let done = encoder.push_pages(batch).unwrap();
            assert!(done > 0 && done <= remaining);
            assert_eq!(encoder.remaining_pages(), remaining - done);
        }
        assert_eq!(encoder.push_pages(1).unwrap(), 0);
        let bytes = encoder.finish().unwrap();
        assert_eq!(bytes, canonical, "batch {batch} changed FVLW v3 bytes");
        let restored = wire::decode_interned(&bytes).unwrap();
        assert_eq!(wire::encode(&restored).unwrap(), wire::encode(&source).unwrap());
    }

    let mut unfinished = wire::InternedEncoder::new(&source);
    assert_eq!(unfinished.push_pages(1).unwrap(), 1);
    assert!(unfinished.finish().is_err());
    let empty = wordcraft_layout::DocLayout::default();
    let encoder = wire::InternedEncoder::new(&empty);
    assert_eq!(encoder.remaining_pages(), 0);
    assert_eq!(encoder.finish().unwrap(), wire::encode_interned(&empty).unwrap());
}

/// Native Linux diagnostic only: FVLW dictionary building and final MessagePack
/// emission use different phases. These are scheduled-thread CPU, not provider CPU.
#[cfg(all(target_os = "linux", feature = "wire-layout-interned"))]
#[test]
#[ignore]
fn profile_interned_batch_finish_cpu() {
    use std::hint::black_box;
    fn cpu_ms() -> f64 {
        let s = std::fs::read_to_string("/proc/thread-self/schedstat").expect("Linux schedstat");
        let ns: u64 = s.split_whitespace().next().unwrap().parse().unwrap();
        ns as f64 * 1e-6
    }
    for (kind, varied) in [("repeated", false), ("varied", true)] {
        let text = if varied {
            (0..960).map(|i| format!("Paragraph {i}: {} {}", "office ".repeat(i % 7 + 1), i * 19)).collect::<Vec<_>>().join("\n")
        } else {
            "A paragraph with office ffi and 123, and a second sentence.\n".repeat(960)
        };
        let mut doc = Document::from_text(&text);
        doc.styles.default_chr.font = Some("Source Sans 3".to_string());
        let source = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
        let expected = wire::encode_interned(&source).unwrap();
        assert!(source.pages.len() >= 30);
        for batch in [1, 3, 8] {
            let mut build = Vec::new();
            let mut finish = Vec::new();
            for i in 0..22 {
                let start = cpu_ms();
                let mut encoder = wire::InternedEncoder::new(black_box(&source));
                while encoder.remaining_pages() > 0 {
                    encoder.push_pages(batch).unwrap();
                }
                let built = cpu_ms();
                let bytes = black_box(encoder.finish().unwrap());
                let finished = cpu_ms();
                assert_eq!(bytes, expected);
                if i >= 6 {
                    build.push(built - start);
                    finish.push(finished - built);
                }
            }
            assert_eq!(build.len(), 16);
            assert_eq!(finish.len(), 16);
            let mean = |samples: &[f64]| samples.iter().sum::<f64>() / samples.len() as f64;
            println!(
                "FVLW_PHASE_CPU fixture={kind} pages={} batch={batch} n=16 build_mean_thread_ms={:.4} finish_mean_thread_ms={:.4} total_mean_thread_ms={:.4}",
                source.pages.len(),
                mean(&build),
                mean(&finish),
                mean(&build) + mean(&finish)
            );
        }
    }
}
