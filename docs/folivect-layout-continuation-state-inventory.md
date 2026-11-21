# Layout pass cross-isolate state inventory — 2026-11-22

Source: crates/layout/src/{lib,pass,ctx,page}.rs at WordCraft b20ec22. This inventory is an implementation investigation, not a claim that these types are serializable today.

## Data that must survive pause and resume
- Global driver: total-page guesses, per-page footnote placement map, note-pass count, repeat count, early-float rewrap set and count, current pass kind.
- Section walk: current section index, last/next block index, current top position, column-balancing flag, and previous block Snap for a future balancing rollback.
- PageBuilder: pages already emitted, current open page/column, y/top/bottom, section geometry, note waiting/carry, previous paragraph spacing, wrap exclusions, floating tables, column balancing and line numbering.
- Ctx: counters, page-field count, equation count, text-box budgets/chains, note hashes, numbering layout, footnote source locations and repeat limits, paragraph state and section grid.
- Snap: state before a block; its pages/items/behind cursor plus counters/field numbering and PageBuilder minus pages. It may be used later to relayout earlier blocks; deleting it breaks balancing.

## Identifiers and transient state
- Do not transfer addresses: Ctx.doc/cache/opts references, HfLinks Arc, PageBuilder.sect/early references, process-local FaceRef/font instance identities and shaping caches.
- Source binding: hash the exact input document bytes and canonical layout options at the gateway, not an unstable HashMap iteration. Keep a public code+font-catalog version identifier with the payload.
- Rehydrate only source-derivable immutable links, fonts and caches; otherwise persist owned canonical values with explicit type/schema version.
- Each restored cursor and vector length must be checked against original document bounds before touching any page items. Validate finite dimensions, all note/page/section indices and total byte limit.

## Gates and risks
- An envelope with arbitrary bytes cannot resume anything. Stage 1 is transport validation only.
- Freezing a pass mid-table, repeated header, float, or paragraph is not supported by the current SectionBlockWalk; single-block CPU can still be unbounded.
- Recording the prior pages and caches has nonzero CPU/memory cost; measure snapshot encode+decode separately and keep producer active CPU below per-call limits.
- Exact re-layout after checkpoint must pass totals, per-page notes, early float rewrap, continuous balanced columns, and same FVLW v3/PDF bytes. Never stitch independent PDFs.
