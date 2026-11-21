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
