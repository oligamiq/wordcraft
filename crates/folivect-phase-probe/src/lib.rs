//! Standalone diagnostic harness for public synthetic documents. Not a Worker ABI.
#![cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
use wordcraft_doc::Document;
use wordcraft_layout::{DocLayout, LayoutCache, LayoutOptions, layout, wire};
use wordcraft_pdf::{PdfOptions, export_layout};

fn failure(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

fn fixture(kind: u32) -> Result<Document, JsValue> {
    // Preserve kinds 0/1 byte-for-byte: they are pinned by existing PDF goldens.
    // Additional fixture kinds stress layout scaling without private documents.
    let count = match kind {
        0 | 1 => 120,
        2 | 3 => 240,
        4 | 5 => 480,
        6 | 7 => 960,
        _ => return Err(failure("Unknown fixture")),
    };
    let text = if kind % 2 == 0 {
        "A paragraph with office ffi and 123, and a second sentence.\n".repeat(count)
    } else {
        (0..count).map(|i| format!("Paragraph {i}: {} {}", "office ".repeat(i % 7 + 1), i * 19)).collect::<Vec<_>>().join("\n")
    };
    let mut doc = Document::from_text(&text);
    doc.styles.default_chr.font = Some("Source Sans 3".into());
    doc.core.created = "2024-05-06T07:08:09Z".into();
    Ok(doc)
}

#[wasm_bindgen]
pub fn prewarm_public_fonts() {
    wordcraft_fonts::FontDb::global().prewarm_static_face_keys();
}

// Split synthetic document construction from the actual layout/decode
// operation, so a parsed-document cost is not mistaken for layout CPU.
#[wasm_bindgen]
pub struct DocumentProbe {
    doc: Option<Document>,
}

#[wasm_bindgen]
impl DocumentProbe {
    pub fn from_fixture(kind: u32) -> Result<DocumentProbe, JsValue> {
        Ok(DocumentProbe { doc: Some(fixture(kind)?) })
    }

    pub fn layout(&mut self) -> Result<Probe, JsValue> {
        let doc = self.doc.take().ok_or_else(|| failure("Document already used"))?;
        let opts = PdfOptions::default();
        let lo = LayoutOptions { hide_deleted: !opts.include_markup, comment_balloons: opts.comments, numeral: opts.numeral, ..Default::default() };
        let lay = layout(&doc, &mut LayoutCache::new(), &lo);
        Ok(Probe { doc, lay })
    }

    pub fn decode(&mut self, codec: u32, packet: &[u8]) -> Result<Probe, JsValue> {
        let lay = match codec {
            1 => wire::decode(packet).map_err(failure)?,
            2 => wire::decode_compact(packet).map_err(failure)?,
            3 => wire::decode_interned(packet).map_err(failure)?,
            _ => return Err(failure("Unknown codec")),
        };
        let doc = self.doc.take().ok_or_else(|| failure("Document already used"))?;
        Ok(Probe { doc, lay })
    }
}

#[wasm_bindgen]
pub struct Probe {
    doc: Document,
    lay: DocLayout,
}

#[wasm_bindgen]
impl Probe {
    pub fn from_fixture(kind: u32) -> Result<Probe, JsValue> {
        let doc = fixture(kind)?;
        let opts = PdfOptions::default();
        let lo = LayoutOptions { hide_deleted: !opts.include_markup, comment_balloons: opts.comments, numeral: opts.numeral, ..Default::default() };
        let lay = layout(&doc, &mut LayoutCache::new(), &lo);
        Ok(Probe { doc, lay })
    }

    pub fn from_packet(kind: u32, codec: u32, packet: &[u8]) -> Result<Probe, JsValue> {
        let doc = fixture(kind)?;
        let lay = match codec {
            1 => wire::decode(packet).map_err(failure)?,
            2 => wire::decode_compact(packet).map_err(failure)?,
            3 => wire::decode_interned(packet).map_err(failure)?,
            _ => return Err(failure("Unknown codec")),
        };
        Ok(Probe { doc, lay })
    }

    pub fn packet(&self, codec: u32) -> Result<Vec<u8>, JsValue> {
        match codec {
            1 => wire::encode(&self.lay).map_err(failure),
            2 => wire::encode_compact(&self.lay).map_err(failure),
            3 => wire::encode_interned(&self.lay).map_err(failure),
            _ => Err(failure("Unknown codec")),
        }
    }

    /// Experimental page-bounded dictionary construction within one WASM call.
    /// This does NOT persist a continuation across Workers; finish still writes
    /// the complete packet. It exists to prove byte equality across batch sizes.
    pub fn packet_batched(&self, pages_per_batch: u32) -> Result<Vec<u8>, JsValue> {
        if pages_per_batch == 0 {
            return Err(failure("Page batch limit must be positive"));
        }
        let mut encoder = wire::InternedEncoder::new(&self.lay);
        while encoder.remaining_pages() > 0 {
            encoder.push_pages(pages_per_batch as usize).map_err(failure)?;
        }
        encoder.finish().map_err(failure)
    }

    pub fn pdf(&self) -> Result<Vec<u8>, JsValue> {
        export_layout(&self.doc, &self.lay, &PdfOptions::default()).map_err(failure)
    }

    pub fn pages(&self) -> usize {
        self.lay.pages.len()
    }
}
