# Durable Layout Continuation — Implementation Plan

> Execute with Superpowers executing-plans, TDD, systematic-debugging, verification-before-completion. The user's A selection authorizes an isolated experiment, not a production rollout.

**Spec:** docs/superpowers/specs/2026-11-22-durable-layout-continuation-design.md
**Base:** WordCraft public fork b20ec2228a3168ee17581ec6b4af455a7a1acdf4; branch folivect/layout-continuation-v1.
**Goal:** Build and validate the safe cross-instance state boundary first, then extend actual pagination into a restartable state machine. Never mislabel stage 1 as full resume.

## Constraints
- No parent Folivect edit, deployment, real DOCX test data, compression, new subscription or scheduled task.
- No heavy local build: CI on GitHub-hosted runner (Rust1.98.1, --locked). Keep tests isolated to new branch.
- Exact existing layout/PDF/FVLW v3 output is the invariant. Explicitly fail on unsupported resume semantics.
- Hash checks detect corruption/mismatch but are not an authentication system; only internal trusted bindings may supply the payload.

## Task 1: source-bound continuation envelope, RED then GREEN
**Files:** crates/layout/src/continuation.rs (new), crates/layout/src/lib.rs, crates/layout/Cargo.toml, .github/workflows/folivect-layout-continuation.yml.
**Interface:** encode(envelope)->Result<Vec<u8>,Error>; decode(bytes, expected_source, expected_opts, section_limit, block_limit)->Result<Envelope,Error>.
1. Add native unit tests first: payload roundtrip; source/options mismatch; version, truncated/extra frame, checksum, zero/oversize, out-of-range cursor and invalid phase.
2. Run targeted CI on a test-only commit to witness expected RED (missing implementation), document exact failure.
3. Implement fixed binary header FVLC v1 using SHA-256, explicit version/cursor/payload bounds and checksum; no pointer serialization, no unsafe.
4. Re-run exact tests off-host and verify GREEN, lints, and unchanged layout goldens.
5. Commit code with evidence and no deployment. Proof is for envelope only.

## Task 2: pagination DTO boundary (not yet demonstrated)
**Files:** crates/layout/src/pass.rs, page.rs, ctx.rs, continuation.rs, targeted fixtures.
1. Inventory each state field with ownership, serializable form, and rebuild strategy; track nonportable FaceRef/Arc/cache fields.
2. RED native test for resume after a section boundary with no prefix replay and exact Page output.
3. GREEN owned pass DTO plus restore and index/finite-value validation; repeat tests across fresh instances.
4. Extend to Snap / column balancing / notes / totals/rewrap with exact complex fixture; prove invalid state rejected.

## Task 3: Worker stage and provider budget (future)
1. Integrate private internal service binding with gateway auth/quota; test request-bound provenance and quota failures.
2. Same-runner ABBA CPU and real Cloudflare per-Worker CPU/p95, memory, call constraints, RPC overhead, failures; no assertion before measurements.
3. Create separate global PDF-finalization-safe chunking design, not PDF concatenation.

## Review focus
- A checksum is not authentication; malformed user input must never become a resume blob.
- Stable on-wire digest must bind complete state header and payload.
- Reject partial, trailing and oversized frames before allocation.
- Never claim 10ms compliance or full Worker restart from envelope-only tests.

## Task 2a: multipass-only portable checkpoint (CURRENT incremental milestone)

**Interface:** `continuation::layout_one_pass(doc, cache, opts, checkpoint, expected_source_digest, expected_options_digest) -> Result<LayoutPassStep, ContinuationError>`.

- RED: `page_total_restarts_only_the_next_pass_across_fresh_caches`, `restart_rejects_wrong_source_options_and_payload_phase`, `unsupported_page_footnote_restart_fails_without_partial_output`. Test-only commit `e2d7af0`, CI `44931398781` initially pending.
- GREEN: serialize only finalized pass driver state (page total guess per section and completed pass count), then restore into a fresh layout pass; do not serialize Ctx or PageBuilder yet.
- Explicitly reject per-page footnote restarts and early-float rewrap until full state schema exists. One pass still processes all blocks; no <=10ms claim.
- Validate canonical wire output after the completion boundary against the unmodified complete layout; validate source/options digest binding and corrupt checkpoint handling.
- Require fresh off-host CI with the pass-boundary tests actually selected by the workflow, not merely compiled/filtered out.
- This milestone is NOT Task 2 complete and does not satisfy the no-replay-in-block acceptance; Task 2b remains the owned Ctx/PageBuilder/Snap state and bounded block advance.

## Task 2b-1: true in-pass plain-body checkpoint (isolated experiment)
- **RED test:** 5f48f694b3f834da4b0acd123aaf46c42e42015f, GitHub Actions #44933608993. Checked expected missing layout_in_resumable_blocks API errors.
- **GREEN target:** continuation::layout_in_resumable_blocks and pass::resume::PlainBlockSnapshot. Capture current page items, paragraph spacing, page/column positions and next-block cursor after each bounded batch. Rehydrate from source plus checkpoint in new LayoutCache, without running past blocks.
- Scope explicitly rejects unsupported tables, inline objects/fields, notes, separate sections, headers, floats, numbering, columns, non-top vertical alignment and textboxes. This is a narrow proven vertical slice, not full Task 2b completion.
- Compare full FVLW layout output for a 140-paragraph multi-page public fixture across 1/3/13/41 block batches and fresh caches. Verify wrong source/options and modified checkpoint are rejected.
- Keep full PageBuilder/Ctx/Snap/column-balancing support and real Cloudflare Workers CPU tests as **subsequent work**. No claim of 10ms compliance; snapshot serialization may dominate CPU as output grows. No compression or production changes.

## Task 2b-2: incremental sealed-page handoff (CURRENT)
- TDD RED `page_delta_tests` first: 220-paragraph multi-page body, new LayoutCache per call, batch sizes 1/5/17/73, returned sealed page first indices strictly consecutive, live FVLC checkpoint stores just one open page and one behind count, final assembled DocLayout index/FVLW v1/v3 identical. Reject wrong source and forged cursor.
- The current first-stage continuation sends all earlier pages repeatedly. This task adds a new `layout_in_page_deltas` API while retaining the old API for compatibility. Each step sends only newly finalized pages as FVLW v1 wire bytes, and an FVLC state with active page, absolute page base and scalar placement metadata. It never builds independent PDFs.
- In each restore, reconstruct immutable context from Document and temporary empty page slots up to the absolute page base so PageBuilder's existing page-count-dependent code retains index semantics. Empty slots are never serialized or treated as output; the coordinator stores sealed page packets independently and concatenates typed Pages before global output serialization. This is a transitional single-section/plain-body design; the O(page_count) temporary placeholder state and header-finalization assumptions need later replacement.
- Validate next block, page base, page counts, finite coordinates, source/options header bindings. Reject unknown or unsupported structures. A SHA checksum is not origin authentication; only trusted internal caller passes bytes.
- Gate: RED CI missing API, GREEN CI native exact equality, regression suite. No parent Folivect edit, compression, production change or Cloudflare 10ms claim.

## Task 2b-3: retry-idempotent sequential page collector (2026-11-22)
- Keep previously proven plain-body incremental page producer unchanged. Implement `PageDeltaCollector` in the continuation module as coordinator-side typed-page collection with a source/options-bound cursor, bounded 10k pages / 64MiB wire receipt budget and a SHA-256 receipt ledger.
- `apply(previous_checkpoint, &PageDeltaStep)` accepts only the exact current checkpoint (or None for first step), monotonic cursor progress, contiguous `first_page`, valid FVLW v1 sealed packets, and checkpoint page_base equal to newly collected page count. Commit only after all checks pass.
- Identical redelivery with a previously accepted predecessor is a read-only `Duplicate`, even after later steps or completion. A conflicting redelivery, skipped step, stale unknown predecessor, malformed packet, excess size or payload mismatch fails without changing collector state.
- Only a terminal `Complete` enables `into_layout`; derive the global index and then serialize ordinary FVLW or final PDF. Never join independently produced PDFs. Static font keys must resolve in the collector process.
- TDD RED `collector_tests` committed before implementation; run on fork CI with feature wire-layout-interned. Integration with a *durable external storage/CAS transaction* and restart across independent Workers remains future work. In-process `&mut` guards mutations only within one coordinator instance, and checksum is not authentication.
