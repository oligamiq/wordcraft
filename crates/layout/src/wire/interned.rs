//! Private uncompressed dictionary packet. Tables are request-local and ordered
//! by first occurrence. No pointers, process-local font IDs or global caches cross the wire.
use super::{Bounded, MAX_WIRE_BYTES};
use crate::para::{
    ClKind, Cluster, Glyph, StyleRun,
    wire::{ParaRecord, ParaRecordRef},
};
use crate::{DocLayout, Page, ParaLayout, Placed};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
    sync::Arc,
};
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

// Write a byte-identical packet using views into the source layout instead of
// cloning each paragraph's lines, rubies, metadata and other large fields.
#[derive(Serialize)]
struct PacketRef<'a> {
    rp: Vec<ResolvedPara>,
    styles: Vec<StyleRun>,
    glyphs: Vec<Vec<Glyph>>,
    clusters: Vec<Vec<Cluster>>,
    paras: Vec<ParaRecordRef<'a>>,
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

// Compare typed fields, including the exact IEEE 754 bit patterns. Hash collisions
// only select candidates: equality must still hold across every field.
trait DirectKey {
    fn hash_fields(&self, h: &mut DefaultHasher);
    fn same_fields(&self, other: &Self) -> bool;
}
impl DirectKey for Glyph {
    fn hash_fields(&self, h: &mut DefaultHasher) {
        let Self { gid, dx, dy } = self;
        (gid, dx.to_bits(), dy.to_bits()).hash(h);
    }
    fn same_fields(&self, other: &Self) -> bool {
        let Self { gid, dx, dy } = self;
        let Self { gid: og, dx: ox, dy: oy } = other;
        gid == og && dx.to_bits() == ox.to_bits() && dy.to_bits() == oy.to_bits()
    }
}
fn kind_key(kind: ClKind) -> (u8, usize) {
    match kind {
        ClKind::Text => (0, 0),
        ClKind::Space => (1, 0),
        ClKind::Tab => (2, 0),
        ClKind::LineBreak => (3, 0),
        ClKind::PageBreak => (4, 0),
        ClKind::ColumnBreak => (5, 0),
        ClKind::Object(id) => (6, id),
        ClKind::Marker => (7, 0),
    }
}
impl DirectKey for Cluster {
    fn hash_fields(&self, h: &mut DefaultHasher) {
        let Self { start, end, adv, kind, style, g0, g1, break_after, obj_h, obj_d, dot } = self;
        (start, end, adv.to_bits(), kind_key(*kind), style, g0, g1, break_after, obj_h.to_bits(), obj_d.to_bits(), dot).hash(h);
    }
    fn same_fields(&self, other: &Self) -> bool {
        let Self { start, end, adv, kind, style, g0, g1, break_after, obj_h, obj_d, dot } = self;
        let Self { start: os, end: oe, adv: oa, kind: ok, style: sty, g0: og0, g1: og1, break_after: ob, obj_h: oh, obj_d: od, dot: ot } = other;
        start == os
            && end == oe
            && adv.to_bits() == oa.to_bits()
            && kind == ok
            && style == sty
            && g0 == og0
            && g1 == og1
            && break_after == ob
            && obj_h.to_bits() == oh.to_bits()
            && obj_d.to_bits() == od.to_bits()
            && dot == ot
    }
}
struct DirectPool<T> {
    values: Vec<Vec<T>>,
    ids: HashMap<u64, Vec<u32>>,
}
impl<T> Default for DirectPool<T> {
    fn default() -> Self {
        Self { values: Vec::new(), ids: HashMap::new() }
    }
}
impl<T: DirectKey + Clone> DirectPool<T> {
    fn intern(&mut self, value: &[T]) -> Result<u32, String> {
        let mut hasher = DefaultHasher::new();
        value.len().hash(&mut hasher);
        for entry in value {
            entry.hash_fields(&mut hasher);
        }
        self.intern_with_hash(value, hasher.finish())
    }

    fn intern_with_hash(&mut self, value: &[T], hash: u64) -> Result<u32, String> {
        if let Some(candidates) = self.ids.get(&hash) {
            for &id in candidates {
                let candidate = lookup(&self.values, id)?;
                if candidate.len() == value.len() && candidate.iter().zip(value).all(|(a, b)| a.same_fields(b)) {
                    return Ok(id);
                }
            }
        }
        let id = u32::try_from(self.values.len()).map_err(|_| "Too many dictionary entries")?;
        self.values.push(value.to_vec());
        self.ids.entry(hash).or_default().push(id);
        Ok(id)
    }
}

#[derive(Default)]
struct Builder<'a> {
    rp: Pool<ResolvedPara>,
    styles: Pool<StyleRun>,
    glyphs: DirectPool<Glyph>,
    clusters: DirectPool<Cluster>,
    paras: Vec<ParaRecordRef<'a>>,
    para_ids: HashMap<*const ParaLayout, u32>,
}
impl<'a> Builder<'a> {
    fn para(&mut self, p: &'a Arc<ParaLayout>) -> Result<u32, String> {
        let key = Arc::as_ptr(p);
        if let Some(id) = self.para_ids.get(&key) {
            return Ok(*id);
        }
        let id = u32::try_from(self.paras.len()).map_err(|_| "Too many paragraphs")?;
        let rp = self.rp.intern(&p.rp)?;
        let styles = p.styles.iter().map(|s| self.styles.intern(s)).collect::<Result<_, _>>()?;
        let glyphs = self.glyphs.intern(&p.glyphs)?;
        let clusters = self.clusters.intern(&p.clusters)?;
        self.paras.push(ParaRecordRef::from_layout(p, rp, styles, glyphs, clusters));
        self.para_ids.insert(key, id);
        Ok(id)
    }
    fn items(&mut self, items: &'a [Placed], depth: usize) -> Result<Vec<Item>, String> {
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
    fn page(&mut self, page: &'a Page) -> Result<PageRecord, String> {
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
    let packet = PacketRef {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_pool_distinguishes_float_bits_and_retains_first_occurrence() {
        let glyph = |dx: f32| Glyph { gid: 12, dx, dy: 0.0 };
        let mut pool = DirectPool::<Glyph>::default();
        assert_eq!(pool.intern(&[glyph(0.0)]).unwrap(), 0);
        assert_eq!(pool.intern(&[glyph(-0.0)]).unwrap(), 1);
        assert_eq!(pool.intern(&[glyph(0.0)]).unwrap(), 0);
        assert_eq!(pool.intern(&[glyph(f32::from_bits(0x7fc00001))]).unwrap(), 2);
        assert_eq!(pool.intern(&[glyph(f32::from_bits(0x7fc00002))]).unwrap(), 3);
        assert_eq!(pool.intern(&[glyph(f32::from_bits(0x7fc00001))]).unwrap(), 2);
        assert_eq!(pool.values.len(), 4);
    }

    #[test]
    fn direct_pool_checks_full_clusters_even_on_hash_collision() {
        let cluster =
            |kind| Cluster { start: 0, end: 1, adv: 1.0, kind, style: 0, g0: 0, g1: 1, break_after: false, obj_h: 0.0, obj_d: 0.0, dot: false };
        let mut pool = DirectPool::<Cluster>::default();
        let a = [cluster(ClKind::Object(3))];
        let b = [cluster(ClKind::Object(5))];
        assert_eq!(pool.intern_with_hash(&a, 10).unwrap(), 0);
        assert_eq!(pool.intern_with_hash(&b, 10).unwrap(), 1);
        assert_eq!(pool.intern_with_hash(&a, 10).unwrap(), 0);
        assert_eq!(pool.values.len(), 2);
    }
}
