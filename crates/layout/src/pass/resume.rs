//! A deliberately restricted but genuine cross-isolate body-block continuation.
//! Only plain one-section print documents are accepted; never silently drop state
//! from tables, notes, floats, columns, headers or per-page fields.

use super::*;
use crate::continuation::ContinuationError;
use wordcraft_doc::para::OBJ;
use wordcraft_doc::section::PageVAlign;

/// The owned state of an open, ordinary one-column page after whole body blocks.
/// It never carries Ctx/PageBuilder references, raw font handles or caches.
/// Page items use the existing wire-layout static font serialization.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PlainBlockSnapshot {
    pub(crate) next_block: usize,
    pages: Vec<Page>,
    y: f32,
    top: f32,
    page_top: f32,
    bottom: f32,
    number: u32,
    orig_bottom: f32,
    prev: Option<(String, bool, f32, bool, bool)>,
    after_break_mark: bool,
    restart_at: Option<u32>,
    line_no: u32,
    page_items_start: usize,
    block_marks: Vec<usize>,
    para_end: Option<(usize, usize, f32)>,
    behind: Vec<usize>,
    max_y: f32,
    content_bottom: f32,
    ctx_page: u32,
}

pub(crate) enum PlainStep {
    Continue(PlainBlockSnapshot),
    Complete(DocLayout),
}

pub(crate) fn supported(doc: &Document, opts: &LayoutOptions) -> bool {
    let s = &doc.last_section;
    opts.view == ViewMode::Print
        && !opts.comment_balloons
        && !doc.body.is_empty()
        && doc.body.len() <= u32::MAX as usize
        && doc.sections().len() == 1
        && doc.parts.is_empty()
        && doc.media.is_empty()
        && doc.numbering.abstracts.is_empty()
        && doc.numbering.nums.is_empty()
        && doc.footnote_separators.is_empty()
        && doc.endnote_separators.is_empty()
        && !doc.settings.mirror_margins
        && s.columns.count == 1
        && s.columns.widths.is_empty()
        && !s.columns.separator
        && s.headers == wordcraft_doc::section::HeaderSet::default()
        && s.footers == wordcraft_doc::section::HeaderSet::default()
        && !s.title_page
        && s.page_num_start.is_none()
        && s.line_numbers.is_none()
        && s.page_borders.is_none()
        && s.valign == PageVAlign::Top
        && s.text_direction == TextDirection::Horizontal
        && !s.rtl
        && doc.body.iter().all(|block| match &**block {
            Block::Para(p) => {
                p.objects.is_empty()
                    && p.section.is_none()
                    && p.props.numbering.is_none()
                    && p.props.frame.is_none()
                    && p.props.drop_cap.is_none()
                    && !p.text.contains(OBJ)
            }
            _ => false,
        })
}

impl PlainBlockSnapshot {
    fn capture(pb: &PageBuilder<'_>, ctx: &Ctx<'_>, next_block: usize) -> Self {
        Self {
            next_block,
            pages: pb.pages.clone(),
            y: pb.y,
            top: pb.top,
            page_top: pb.page_top,
            bottom: pb.bottom,
            number: pb.number,
            orig_bottom: pb.orig_bottom,
            prev: pb.prev.clone(),
            after_break_mark: pb.after_break_mark,
            restart_at: pb.restart_at,
            line_no: pb.line_no,
            page_items_start: pb.page_items_start,
            block_marks: pb.block_marks.clone(),
            para_end: pb.para_end,
            behind: pb.behind.clone(),
            max_y: pb.max_y,
            content_bottom: pb.content_bottom,
            ctx_page: ctx.fields.page,
        }
    }

    fn validate(&self, doc: &Document) -> Result<(), ContinuationError> {
        let current = self.pages.last().ok_or(ContinuationError::InvalidState)?;
        let valid_positions =
            [self.y, self.top, self.page_top, self.bottom, self.orig_bottom, self.max_y, self.content_bottom].into_iter().all(f32::is_finite);
        let valid_prev = self.prev.as_ref().is_none_or(|(_, _, space, _, _)| space.is_finite());
        let valid_end = self.para_end.is_none_or(|(_, _, y)| y.is_finite());
        if self.next_block == 0
            || self.next_block >= doc.body.len()
            || self.pages.len() != self.behind.len()
            || self.pages.len() > 10_000
            || !valid_positions
            || !valid_prev
            || !valid_end
            || self.number != current.number
            || current.section != 0
            || self.page_items_start > current.items.len()
            || self.behind.iter().any(|&n| n > 100_000)
            || self.block_marks.iter().any(|&m| m > current.items.len())
            || self.ctx_page == 0
        {
            return Err(ContinuationError::InvalidState);
        }
        Ok(())
    }

    fn restore<'p>(self, pb: &mut PageBuilder<'p>, ctx: &mut Ctx<'_>) {
        pb.pages = self.pages;
        pb.y = self.y;
        pb.top = self.top;
        pb.page_top = self.page_top;
        pb.bottom = self.bottom;
        pb.number = self.number;
        pb.orig_bottom = self.orig_bottom;
        pb.prev = self.prev;
        pb.after_break_mark = self.after_break_mark;
        pb.restart_at = self.restart_at;
        pb.line_no = self.line_no;
        pb.page_items_start = self.page_items_start;
        pb.block_marks = self.block_marks;
        pb.para_end = self.para_end;
        pb.behind = self.behind;
        pb.max_y = self.max_y;
        pb.content_bottom = self.content_bottom;
        ctx.fields.page = self.ctx_page;
    }
}

/// Reconstruct the immutable environment in a fresh isolate, then place only
/// [next_block, next_block + max_blocks). Earlier block layouts are never run.
pub(crate) fn step(
    doc: &Document,
    cache: &mut LayoutCache,
    opts: &LayoutOptions,
    saved: Option<PlainBlockSnapshot>,
    max_blocks: usize,
) -> Result<PlainStep, ContinuationError> {
    if max_blocks == 0 {
        return Err(ContinuationError::OutOfBounds);
    }
    if !supported(doc, opts) {
        return Err(ContinuationError::Unsupported);
    }
    if let Some(s) = &saved {
        s.validate(doc)?;
    }

    let notes = HashMap::new();
    let totals = Totals::first_guess(1);
    let early = EarlyFloats::new();
    reset_cache(doc, cache, opts);
    let mut ctx = new_ctx(doc, cache, opts, (&notes, &totals));
    let hf = Arc::clone(&ctx.hf);
    let sections = &hf.sections;
    let mode = Mode::of(opts.view);
    ctx.mirror = false;
    (ctx.stacked_marks, ctx.quiet_marks) = section_marks(doc, opts, sections);
    let sect = sections.first().map(|(_, s)| *s).ok_or(ContinuationError::Unsupported)?;
    let tops = section_tops(&mut ctx, &hf, (0, sect), false);
    let sec = Sec { si: 0, sect, start: SectionStart::NextPage, tops };
    let mut pb = new_page_builder(sect, &early, false, false);
    begin_section_state(&mut ctx, &mut pb, &sec, &totals, false);
    start_section_page(&mut ctx, &mut pb, &hf, &sec, (mode, 0));
    pb.foot_top = sec.tops.foot;
    if prepare_section_end(&mut pb, sections, (0, false)) {
        return Err(ContinuationError::Unsupported);
    }

    let first = saved.as_ref().map_or(0, |s| s.next_block);
    if let Some(saved) = saved {
        saved.restore(&mut pb, &mut ctx);
    }
    let end = first.saturating_add(max_blocks).min(doc.body.len());
    for block in first..end {
        pb.mark_block();
        place_block(&mut ctx, &mut pb, &None, block, sec.tops.body);
    }

    // These side effects need additional owned DTO state, so reject them rather
    // than producing output that diverges subtly at a later checkpoint.
    if ctx.uses_totals
        || !ctx.floats_above.is_empty()
        || ctx.boxes.total() != 0
        || ctx.eq_count != 0
        || ctx.header_repeats != 0
        || !ctx.table_keys.is_empty()
        || !ctx.fields.page_from.is_empty()
    {
        return Err(ContinuationError::Unsupported);
    }

    if end < doc.body.len() {
        return Ok(PlainStep::Continue(PlainBlockSnapshot::capture(&pb, &ctx, end)));
    }

    let mut pages = finish_pages(pb, doc, sections, false);
    ctx.grid = para::Grid::default();
    if fill_headers_footers(&mut ctx, &mut pages, sections, (&totals, false)).is_some() {
        return Err(ContinuationError::Unsupported);
    }
    evict_unused(ctx.cache);
    Ok(PlainStep::Complete(DocLayout { index: index_lines(&pages), pages, ms: 0.0, text_boxes: 0 }))
}
