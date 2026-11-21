# Folivect per-Worker CPU continuation design (experiment only)

Status: **incomplete; no Cloudflare runtime proof**. The public WordCraft
fork experiment is not a deployed Folivect interface.

## Measured constraints

Standalone Node/WASM, reused isolate, 3 warmups and 12 measured requests,
for a 960-paragraph synthetic document:
- Producer full layout + interned v3 encode: 38.88–39.90 ms thread CPU.
- Layout alone: 22.17–26.28 ms.
- Dictionary encoding alone: 11.92–16.67 ms.
- Consumer decode + PDF: 15.63–20.76 ms.

This is not provider-billed Cloudflare CPU. Network waiting does not count
toward active CPU; serialization, protocol work and copying do. No
compression is in scope.

## Valid and invalid boundaries

The existing layout() repeats layout_pass() as page totals, per-page note
numbers, and early float wrap placements converge. It preserves an open
page/column, footnote carry, field/counter state, sectional page geometry,
column balancing snapshots, and caches. A paragraph or page is NOT an
independent layout unit. Future content can invalidate earlier pages.

A resumable layout pass must retain or deterministically reconstruct:
- Source document/layout options/catalog version and source hash.
- Section and block cursor, note/rewrap/total pass counters and state.
- Numbering, fields, note restart, page totals and cross-references.
- Open pages/columns, float exclusion areas, notes, balancing snapshots.
- Header/footer links, text boxes, cache environment and font identities.

Raw references, Rust pointer addresses and process-local font IDs must
never be transferred across Worker isolates. Reject mismatched or corrupt
continuation state before processing.

## Current implementation

The experimental wire::InternedEncoder borrows a **completed** DocLayout
within a single Rust session, and advances dictionary construction by up
to N pages at a time. It retains request-local first-seen IDs and follows
the original page order. Zero batch size, unfinished finalization, and
previously failed sessions error rather than masquerade as completion.
The original encode_interned() uses the same code path, unbounded.

This does NOT split the layout pass, serialize a continuation, split the
final whole-packet serializer, or guarantee any Worker request budget.

## Next acceptance gates

1. Test multiple page batch sizes against exact FVLW v3 byte output,
   restored index and pinned complete PDFs. Extend to tables, floats,
   headers, notes, section breaks and page-total fields.
2. Measure each batch and final packet serialization CPU separately at
   5/10/20/39 pages. If final serialization exceeds 10ms, design a
   streaming writer with durable output chunks.
3. Implement an isolate-independent, versioned continuation format and
   prove arbitrary resume points preserve deterministic output. All
   dependencies and cache state must be validated on resume.
4. Add a bounded layout-pass state machine respecting multipass convergence.
5. Add global PDF resource/tag/finalization-safe staging. Never simply
   concatenate independently generated PDFs.
6. Measure real Cloudflare Workers active CPU including cold start,
   JS/RPC overhead, snapshot rehydration, and high percentiles before any
   deployment or claims of meeting the provider limit.

Only the active CPU cost of the given Worker invocation is the primary
metric. Mean CPU below the configured limit does not prove compliance.
