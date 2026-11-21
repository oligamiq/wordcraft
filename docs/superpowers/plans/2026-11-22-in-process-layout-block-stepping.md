# In-Process Layout Block Stepping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow a WordCraft layout pass to be progressed in bounded numbers of body blocks while preserving exact pagination and FVLW v3 output.

**Architecture:** Keep the existing document/page-note/float/total multi-pass driver. Extract its section block walk into a cursor holding the current block, optional balancing snapshot, and section bounds. Advance it in batches in the *same Rust lifetime* while preserving the Ctx and PageBuilder. No cross-Worker persistence, CPU budget guarantee, or PDF splitting is claimed.

**Tech Stack:** Rust1.98.1, wordcraft-layout; CI GitHub Actions on public fork; exact FVLW v3 comparison.

**Spec:** docs/folivect-worker-cpu-continuations.md

## Global Constraints

- Existing layout() behavior and serialization unchanged.
- Only an experimental API behind wire-layout-interned, rejecting a zero batch.
- No compression, parent repository change, or deployment.
- Document may require whole-pass repetition for NUMPAGES, footnote restart, early floats, and balancing. Keep state intact across chunks.
- Local host safety hold: do not run heavy local compilation; use fork CI.
- Test source includes synthetic complex fixtures, static bundled font, and byte-identical FVLW v3.

## Review Focus

- Empty document and last-block boundaries cannot spin on a zero-length chunk.
- Continuous section balancing snapshots must remain live through a chunk boundary.
- Early-float rewrap must continue to re-run with the same max-rewrap behavior.
- Per-page footnote restart and document total-page fields may trigger multiple passes.
- "In-process chunks" are not durable checkpoints; unsupported semantics must be explicit.

---

### Task 1: Exact-output red test

**Files:** crates/layout/src/tests.rs; .github/workflows/folivect-wasm-phase-probe.yml.

- [x] Add regression block_batched_layout_preserves_multipass_pagination_and_exact_wire covering totals and per-page notes, continuous 2-column sections, and early-float rewrap.
- [x] Enroll the test in the fork CI job.
- [x] Verify red on CI44923325572: expected Rust E0425 at tests.rs:12898 and12903 for absent layout_in_block_batches, not unrelated failure.

### Task 2: In-process section block walker

**Files:** crates/layout/src/pass.rs, crates/layout/src/lib.rs, crates/layout/src/tests.rs.

**Interfaces:** pub fn layout_in_block_batches(doc:&Document,cache:&mut LayoutCache,opts:&LayoutOptions,max_blocks:usize)->Result<DocLayout,String> behind wire-layout-interned; chunk size must be positive.

- [ ] Extract SectionBlockWalk retaining (block,last), balanced-column Snap, and base vertical positioning.
- [ ] Implement advance(max_blocks) with identical pre/post state updates for each block and finish balancing exactly once.
- [ ] Keep layout() calling the same pass code unbounded; explicit bounded API calls the same pass driver but with chunk size.
- [ ] Add chunk_size plumbing to layout_pass and update its direct test invocations.
- [ ] Run existing CI, verify the new test now passes and existing native/WASM golden tests still pass. Fix root causes, not snapshots.

### Task 3: Evidence and handoff

**Files:** docs/folivect-worker-cpu-continuations.md; Drive CURRENT/Checkpoint.

- [ ] Record the supported boundary and lack of persistence/provider CPU verification.
- [ ] Verify fork clean and remote HEAD matches local; no parent/private changes.
- [ ] Record exact raw conversation and final answer in Google Drive, read back.
