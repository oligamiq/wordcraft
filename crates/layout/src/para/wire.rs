//! Explicit paragraph representation for the private, uncompressed v3 layout packet.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct ParaRecord {
    pub(crate) rp: u32,
    pub(crate) styles: Vec<u32>,
    pub(crate) glyphs: u32,
    pub(crate) clusters: u32,
    lines: Vec<Line>,
    label: Option<Label>,
    height: f32,
    text_len: usize,
    joined_cluster_starts: Vec<usize>,
    has_page_fields: bool,
    notes: Vec<(usize, u32)>,
    issues: Vec<(usize, usize, bool)>,
    drop_cap: Option<(usize, u8, f32)>,
    hyph_after: Vec<u32>,
    bidi_levels: Vec<u8>,
    drop_cap_w: f32,
    maths: Vec<(usize, Arc<crate::math::MathLayout>)>,
    rubies: Vec<RubyLayout>,
    displays: Vec<(usize, wordcraft_doc::math::MathJc)>,
    ptabs: Vec<(usize, PTabAlign, PTabBase, TabLeader)>,
    left_out: Vec<std::ops::Range<usize>>,
    shown: Vec<(usize, String, Vec<std::ops::Range<usize>>)>,
    enclosures: Vec<(usize, Enclosed)>,
    auto_space: Vec<(usize, f32)>,
}

/// Borrowed serialization view. Keep the wire fields/order equal to
/// ParaRecord but avoid duplicating line data and optional drawing metadata.
#[derive(Serialize)]
pub(crate) struct ParaRecordRef<'a> {
    rp: u32,
    styles: Vec<u32>,
    glyphs: u32,
    clusters: u32,
    lines: &'a Vec<Line>,
    label: &'a Option<Label>,
    height: f32,
    text_len: usize,
    joined_cluster_starts: &'a Vec<usize>,
    has_page_fields: bool,
    notes: &'a Vec<(usize, u32)>,
    issues: &'a Vec<(usize, usize, bool)>,
    drop_cap: Option<(usize, u8, f32)>,
    hyph_after: &'a Vec<u32>,
    bidi_levels: &'a Vec<u8>,
    drop_cap_w: f32,
    maths: &'a Vec<(usize, Arc<crate::math::MathLayout>)>,
    rubies: &'a Vec<RubyLayout>,
    displays: &'a Vec<(usize, wordcraft_doc::math::MathJc)>,
    ptabs: &'a Vec<(usize, PTabAlign, PTabBase, TabLeader)>,
    left_out: &'a Vec<std::ops::Range<usize>>,
    shown: &'a Vec<(usize, String, Vec<std::ops::Range<usize>>)>,
    enclosures: &'a Vec<(usize, Enclosed)>,
    auto_space: &'a Vec<(usize, f32)>,
}

impl<'a> ParaRecordRef<'a> {
    pub(crate) fn from_layout(p: &'a ParaLayout, rp: u32, styles: Vec<u32>, glyphs: u32, clusters: u32) -> Self {
        // Complete destructuring intentionally breaks compilation if any new field is added.
        let ParaLayout {
            rp: _,
            styles: _,
            glyphs: _,
            clusters: _,
            lines,
            label,
            height,
            text_len,
            joined_cluster_starts,
            has_page_fields,
            notes,
            issues,
            drop_cap,
            hyph_after,
            bidi_levels,
            drop_cap_w,
            maths,
            rubies,
            displays,
            ptabs,
            left_out,
            shown,
            enclosures,
            auto_space,
        } = p;
        Self {
            rp,
            styles,
            glyphs,
            clusters,
            lines,
            label,
            height: *height,
            text_len: *text_len,
            joined_cluster_starts,
            has_page_fields: *has_page_fields,
            notes,
            issues,
            drop_cap: *drop_cap,
            hyph_after,
            bidi_levels,
            drop_cap_w: *drop_cap_w,
            maths,
            rubies,
            displays,
            ptabs,
            left_out,
            shown,
            enclosures,
            auto_space,
        }
    }
}

impl ParaRecord {
    pub(crate) fn from_layout(p: &ParaLayout, rp: u32, styles: Vec<u32>, glyphs: u32, clusters: u32) -> Self {
        // Exhaustive match ensures additions to ParaLayout cannot silently disappear.
        let ParaLayout {
            rp: _,
            styles: _,
            glyphs: _,
            clusters: _,
            lines,
            label,
            height,
            text_len,
            joined_cluster_starts,
            has_page_fields,
            notes,
            issues,
            drop_cap,
            hyph_after,
            bidi_levels,
            drop_cap_w,
            maths,
            rubies,
            displays,
            ptabs,
            left_out,
            shown,
            enclosures,
            auto_space,
        } = p;
        Self {
            rp,
            styles,
            glyphs,
            clusters,
            lines: lines.clone(),
            label: label.clone(),
            height: *height,
            text_len: *text_len,
            joined_cluster_starts: joined_cluster_starts.clone(),
            has_page_fields: *has_page_fields,
            notes: notes.clone(),
            issues: issues.clone(),
            drop_cap: *drop_cap,
            hyph_after: hyph_after.clone(),
            bidi_levels: bidi_levels.clone(),
            drop_cap_w: *drop_cap_w,
            maths: maths.clone(),
            rubies: rubies.clone(),
            displays: displays.clone(),
            ptabs: ptabs.clone(),
            left_out: left_out.clone(),
            shown: shown.clone(),
            enclosures: enclosures.clone(),
            auto_space: auto_space.clone(),
        }
    }

    pub(crate) fn restore(self, rp: ResolvedPara, styles: Vec<StyleRun>, glyphs: Vec<Glyph>, clusters: Vec<Cluster>) -> ParaLayout {
        let Self {
            rp: _,
            styles: _,
            glyphs: _,
            clusters: _,
            lines,
            label,
            height,
            text_len,
            joined_cluster_starts,
            has_page_fields,
            notes,
            issues,
            drop_cap,
            hyph_after,
            bidi_levels,
            drop_cap_w,
            maths,
            rubies,
            displays,
            ptabs,
            left_out,
            shown,
            enclosures,
            auto_space,
        } = self;
        ParaLayout {
            rp,
            styles,
            glyphs,
            clusters,
            lines,
            label,
            height,
            text_len,
            joined_cluster_starts,
            has_page_fields,
            notes,
            issues,
            drop_cap,
            hyph_after,
            bidi_levels,
            drop_cap_w,
            maths,
            rubies,
            displays,
            ptabs,
            left_out,
            shown,
            enclosures,
            auto_space,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayoutCache, LayoutOptions, Placed, layout};
    use wordcraft_doc::Document;

    #[test]
    fn borrowed_paragraph_is_byte_identical_to_owned_record() {
        let doc = Document::from_text("Paragraph with ffi and 123\nLine two");
        let pages = layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
        let para = pages
            .pages
            .iter()
            .flat_map(|p| &p.items)
            .find_map(|item| match item {
                Placed::Lines { para, .. } => Some(para.as_ref()),
                _ => None,
            })
            .expect("public synthetic paragraph");
        let ids = (0..para.styles.len()).map(|i| u32::try_from(i).unwrap()).collect::<Vec<_>>();
        let owned = ParaRecord::from_layout(para, 1, ids.clone(), 2, 3);
        let borrowed = ParaRecordRef::from_layout(para, 1, ids, 2, 3);
        let mut a = Vec::new();
        owned.serialize(&mut rmp_serde::Serializer::new(&mut a).with_struct_map().with_human_readable()).unwrap();
        let mut b = Vec::new();
        borrowed.serialize(&mut rmp_serde::Serializer::new(&mut b).with_struct_map().with_human_readable()).unwrap();
        assert_eq!(a, b, "v3 packet cannot change with borrowed serialization");
    }
}
