//! Private uncompressed dictionary packet. Tables are request-local and ordered
//! by first occurrence. No pointers, process-local font IDs or global caches cross the wire.
use super::{Bounded, MAX_WIRE_BYTES};
use crate::para::{Cluster, Glyph, StyleRun, wire::ParaRecord};
use crate::{DocLayout, Page, ParaLayout, Placed};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Cursor, sync::Arc};
use wordcraft_doc::{Path, StoryRef, props::TextDirection, resolve::ResolvedPara};
use wordcraft_geom::{Rect, Spin};

const MAGIC: &[u8] = b"FVLW\x03";
const MAX_EXPANDED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Packet {
    rp: Vec<ResolvedPara>,
    styles: Vec<StyleRun>,
    glyphs: Vec<Vec<Glyph>>,
    clusters: Vec<Vec<Cluster>>,
    paras: Vec<ParaRecord>,
    pages: Vec<PageRecord>,
}

#[derive(Serialize, Deserialize)]
struct PageRecord {
    meta: Page,
    items: Vec<Item>,
    header: Vec<Item>,
    footer: Vec<Item>,
}

#[derive(Serialize, Deserialize)]
enum Item {
    Lines { story: StoryRef, path: Path, para: u32, l0: usize, l1: usize, x: f32, y: f32, turn: TextDirection },
    Clip { rect: Rect, spin: Spin, items: Vec<Item> },
    Other(Placed),
}

// Compare full serialized values, not hashes alone; float bits, optional fields,
// and font identities retain the same semantics as the v2 representation.
struct Pool<T> {
    values: Vec<T>,
    ids: HashMap<Vec<u8>, u32>,
}
impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self { values: Vec::new(), ids: HashMap::new() }
    }
}
impl<T: Serialize + Clone> Pool<T> {
    fn intern(&mut self, value: &T) -> Result<u32, String> {
        let mut bounded = Bounded(Vec::new());
        value.serialize(&mut rmp_serde::Serializer::new(&mut bounded).with_struct_map().with_human_readable()).map_err(|e| e.to_string())?;
        let key = bounded.0;
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        let id = u32::try_from(self.values.len()).map_err(|_| "Too many dictionary entries")?;
        self.values.push(value.clone());
        self.ids.insert(key, id);
        Ok(id)
    }
}

#[derive(Default)]
struct Builder {
    rp: Pool<ResolvedPara>,
    styles: Pool<StyleRun>,
    glyphs: Pool<Vec<Glyph>>,
    clusters: Pool<Vec<Cluster>>,
    paras: Vec<ParaRecord>,
    para_ids: HashMap<*const ParaLayout, u32>,
}
impl Builder {
    fn para(&mut self, p: &Arc<ParaLayout>) -> Result<u32, String> {
        let key = Arc::as_ptr(p);
        if let Some(id) = self.para_ids.get(&key) {
            return Ok(*id);
        }
        let id = u32::try_from(self.paras.len()).map_err(|_| "Too many paragraphs")?;
        let rp = self.rp.intern(&p.rp)?;
        let styles = p.styles.iter().map(|s| self.styles.intern(s)).collect::<Result<_, _>>()?;
        let glyphs = self.glyphs.intern(&p.glyphs)?;
        let clusters = self.clusters.intern(&p.clusters)?;
        self.paras.push(ParaRecord::from_layout(p, rp, styles, glyphs, clusters));
        self.para_ids.insert(key, id);
        Ok(id)
    }
    fn items(&mut self, items: &[Placed], depth: usize) -> Result<Vec<Item>, String> {
        if depth > 64 {
            return Err("Layout nesting exceeds limit".into());
        }
        items
            .iter()
            .map(|item| {
                Ok(match item {
                    Placed::Lines { story, path, para, l0, l1, x, y, turn } => {
                        Item::Lines { story: *story, path: path.clone(), para: self.para(para)?, l0: *l0, l1: *l1, x: *x, y: *y, turn: *turn }
                    }
                    Placed::Clip { rect, spin, items } => Item::Clip { rect: *rect, spin: *spin, items: self.items(items, depth + 1)? },
                    other => Item::Other(other.clone()),
                })
            })
            .collect()
    }
    fn page(&mut self, page: &Page) -> Result<PageRecord, String> {
        let mut meta = page.clone();
        meta.items.clear();
        meta.header.clear();
        meta.footer.clear();
        Ok(PageRecord { meta, items: self.items(&page.items, 0)?, header: self.items(&page.header, 0)?, footer: self.items(&page.footer, 0)? })
    }
}

pub fn encode_interned(layout: &DocLayout) -> Result<Vec<u8>, String> {
    let mut builder = Builder::default();
    let pages = layout.pages.iter().map(|p| builder.page(p)).collect::<Result<_, _>>()?;
    let packet = Packet {
        rp: builder.rp.values,
        styles: builder.styles.values,
        glyphs: builder.glyphs.values,
        clusters: builder.clusters.values,
        paras: builder.paras,
        pages,
    };
    let mut writer = Bounded(MAGIC.to_vec());
    packet.serialize(&mut rmp_serde::Serializer::new(&mut writer).with_struct_map().with_human_readable()).map_err(|e| e.to_string())?;
    Ok(writer.0)
}

fn lookup<T>(table: &[T], id: u32) -> Result<&T, String> {
    table.get(id as usize).ok_or_else(|| "Invalid layout dictionary reference".into())
}
fn restore_items(items: Vec<Item>, paras: &[Arc<ParaLayout>], depth: usize) -> Result<Vec<Placed>, String> {
    if depth > 64 {
        return Err("Layout nesting exceeds limit".into());
    }
    items
        .into_iter()
        .map(|item| {
            Ok(match item {
                Item::Lines { story, path, para, l0, l1, x, y, turn } => {
                    let para = lookup(paras, para)?;
                    if l0 > l1 || l1 > para.lines.len() {
                        return Err("Invalid line range".into());
                    }
                    Placed::Lines { story, path, para: Arc::clone(para), l0, l1, x, y, turn }
                }
                Item::Clip { rect, spin, items } => Placed::Clip { rect, spin, items: restore_items(items, paras, depth + 1)? },
                Item::Other(Placed::Lines { .. } | Placed::Clip { .. }) => return Err("Noncanonical layout item".into()),
                Item::Other(other) => other,
            })
        })
        .collect()
}
pub fn decode_interned(input: &[u8]) -> Result<DocLayout, String> {
    if input.len() > MAX_WIRE_BYTES {
        return Err("Layout exceeds wire limit".into());
    }
    let body = input.strip_prefix(MAGIC).ok_or("Invalid interned layout version")?;
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(body)).with_human_readable();
    decoder.set_max_depth(128);
    let packet = Packet::deserialize(&mut decoder).map_err(|e| e.to_string())?;
    if decoder.get_ref().position() != body.len() as u64 {
        return Err("Trailing bytes in interned layout".into());
    }

    let Packet { rp, styles, glyphs, clusters, paras, pages } = packet;
    let mut expanded = 0usize;
    let mut restored = Vec::new();
    // Cap the cumulative fixed-size array expansion before cloning. This is
    // a trusted internal format, not a decoder for arbitrary public payloads.
    for p in paras {
        let g = lookup(&glyphs, p.glyphs)?;
        let c = lookup(&clusters, p.clusters)?;
        expanded = expanded
            .saturating_add(g.len().saturating_mul(std::mem::size_of::<Glyph>()))
            .saturating_add(c.len().saturating_mul(std::mem::size_of::<Cluster>()))
            .saturating_add(p.styles.len().saturating_mul(std::mem::size_of::<StyleRun>()));
        if expanded > MAX_EXPANDED_BYTES {
            return Err("Expanded layout exceeds limit".into());
        }
        let r = lookup(&rp, p.rp)?.clone();
        let s = p.styles.iter().map(|id| lookup(&styles, *id).cloned()).collect::<Result<_, _>>()?;
        restored.push(Arc::new(p.restore(r, s, g.clone(), c.clone())));
    }
    let pages = pages
        .into_iter()
        .map(|p| {
            let PageRecord { mut meta, items, header, footer } = p;
            if !meta.items.is_empty() || !meta.header.is_empty() || !meta.footer.is_empty() {
                return Err("Noncanonical page metadata".into());
            }
            meta.items = restore_items(items, &restored, 0)?;
            meta.header = restore_items(header, &restored, 0)?;
            meta.footer = restore_items(footer, &restored, 0)?;
            Ok(meta)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let index = crate::pass::index_lines(&pages);
    Ok(DocLayout { pages, index, ..Default::default() })
}
