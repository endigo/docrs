# Rust port handoff

## User decisions and authorization

The user selected a standalone Rust library and CLI, then requested an Orca-created worktree using Claude models. This is an implementation handoff. Use Claude Opus as lead; Claude subagents may be used when useful, with isolated ownership of files and separate Cargo target directories for concurrent builds.

## Scope

Own the Rust port described in `docs/port-plan.mdx`. Start by completing M0 and M1, then continue toward M3 as the recommended first usable release. The full roadmap through M7 remains in scope. Do not ask for confirmation between routine milestones; report evidence and keep progress reviewable. Any first-release scope choice beyond the default M3 remains open and must not be presented as already approved.

## Grounding

Upstream: https://github.com/paper-instruments/paper-docx, inspected commit `3c0f9a5`, package 0.2.1. A read-only local clone exists at `/tmp/paper-docx-review.lkLyZs/repo`; do not rely on that temporary location for reproducibility. Establish a pinned reference checkout under ignored `.reference/` or a similarly explicit test-only path.

Baseline on Python 3.13: pytest had 3,003 passed, 7 skipped and 42 failures, all caused by `/opt/homebrew/bin/soffice` pointing to a missing `/Applications/LibreOffice.app`. Behave passed 67 features / 650 scenarios / 1,856 steps. A broken `soffice` launcher must not count as a successful independent-loader run. Do not change unrelated machine software or claim that gate passed without a working loader.

## Begin concretely

1. Read the plan and inspect upstream contracts, tests and fixture manifest. Define the Rust public capability ledger and cross-language case format before feature expansion.
2. Scaffold a Cargo workspace with `paper-docx`, `paper-docx-cli` and a test-only compatibility harness. Read current official dependency docs and pin tested versions. Keep error classes, resource-limit policy and Unicode offset semantics explicit.
3. Prove guarded ZIP/OPC reads, namespace and unknown-content preservation, no-op byte identity, changed-part budgets, atomic saves and late-failure behavior before general editing work.
4. Add shared stories/text projection, normalization/source maps, local handle freshness and transactional safe replacement. Carry tests over as behavioral cases rather than translating Python mock internals.
5. Continue in dependency order. Track milestone progress and meaningful validation in docs. Add appropriate MIT attribution before adapting upstream source, fixtures or templates.

## Critical design constraints

- Original package and XML arenas are the preservation authority; do not deserialize into a partial model then discard unknown markup.
- Retain lexical namespace scopes and QName-valued attribute semantics; use non-reused node IDs and local freshness evidence across transactions.
- Python scalar offsets differ from Rust byte offsets; reproduce normalization, full case folding and whitespace rules with source mapping.
- Failed edits preserve document state and handles; failed saves preserve existing destination files. A successful edit and later failed save are separate operations.
- No-op replacement leaves handles reusable. Successful text-changing replacement consumes the selected span and its aliases.
- Explicit resource-budget errors are policy outcomes, never mislabeled as malformed-document evidence.
- Compare eventually proves accept/reject algebra on semantic package structure. Composition reconciles dependency graphs rather than copying XML alone.

## Boundaries

Work only in the new repository/worktree. Do not modify WordCraft or the upstream checkout. Do not push, publish packages or create a remote unless the user asks. Commit cohesive verified changes locally when a safe author identity is available; never invent the user's identity.
