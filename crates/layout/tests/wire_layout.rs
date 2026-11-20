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
