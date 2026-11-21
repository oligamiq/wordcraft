# Durable Layout Continuation — Design (2026-11-22)

## Decision and intent
User selected A: safely resume WordCraft pagination across isolated Cloudflare Workers while retaining exact full-document output. This is a staged implementation in the public WordCraft fork only; Folivect parent and production are untouched.

## Goals
- Bounded, validated, versioned, source-and-options-bound continuation format with corruption/mismatch rejection.
- Eventually pause and resume actual multipass layout across distinct WASM instances without recomputing already-finished blocks or retaining process-local pointers.
- Preserve complete DocLayout page ordering, FVLW v3 exact bytes, index, and canonical PDF, including totals, footnotes, floats, continuous sections and column balancing.
- Measure active CPU per invocation and RPC count; do not assert unmeasured compliance.

## Exclusions and constraints
- No compression in this experiment; no independently generated PDF page joins; no deployment.
- No Rust pointers, temporary font FaceRef identities, or allocator handles in wire representation.
- Xubuntu heavy build hold: use off-host CI; no parent-submodule update.
- Expensive single blocks and finalization remain unsplit until separately addressed.

## Architecture / interfaces
- Stage 1: LayoutContinuationEnvelope: magic, schema version, source SHA-256, options SHA-256, pass/section/block cursor, bounded payload, integrity SHA-256. Fail closed for unknown version, invalid bounds, mismatches, oversized payload and tampering. This is NOT a resumable layout engine.
- Stage 2: owned LayoutPassStateV1 DTO for Ctx, PageBuilder, Snap, SectionBlockWalk, multi-pass totals, footnote pages, early floats and endnotes. Rebuild HfLinks, font lookup and caches from source with canonical keys; validate indices and finite values.
- Stage 3: step driver restoring, advancing a positive bounded count of body blocks, saving unfinished state, or finalizing one pass. Multi-pass repeat/rewrap transitions remain exact.
- Stage 4: private Worker transport, separate PDF-finalization design and actual Cloudflare CPU/memory/latency/error measurements; gateway retains auth/quota.

## Alternatives considered
A selected: source-bound full-state continuation. B alone (monolithic optimizations) cannot bound large docs, though small gains should be preserved. C (fixed document cap) fails the product goal. Raw memory snapshots and redoing the entire prefix at every boundary are rejected.

## Acceptance
Stage 1 TDD: roundtrip plus source/options/version/truncation/tampering/cursor/size failures. Stage 2+ TDD: genuinely independent WASM restore and exact output. Only provider measurements can prove CPU budget compliance.
