# Stage 1 stabilization and integration

This repository is now in a temporary **freeze / consolidation phase**. The purpose of this phase is to convert a large set of stacked experimental pull requests into a small number of reviewable integration lanes without continuing open-ended feature growth.

## Freeze policy

Until Stage 1 is integrated or explicitly abandoned, changes on the active integration lanes should be limited to:

1. correctness fixes for behavior already implemented in the lane;
2. regression tests for standards conformance, security boundaries, redirect/fetch state machines, parsing, lifecycle, and portability;
3. CI/build fixes needed to keep exact heads reproducible;
4. removal of duplicated or superseded implementation paths;
5. documentation that narrows or clarifies guarantees;
6. conflict-resolution work needed to combine the retained integration lanes.

Do not add unrelated Web APIs, CSS features, DOM features, new rendering subsystems, or new protocol families during stabilization.

## Stage 1 integration lanes

### Lane A — Fetch / navigation / cookies / referrer / CORS

Primary umbrella: PR #267 (`agent/fetch-preflight-max-age-delta-seconds`).

This lane represents the current cumulative Fetch-oriented stack and is the first integration candidate. Its covered surface includes:

- browser/session cookie policy and SameSite handling;
- HSTS-aware network/session request processing inherited by the Fetch stack;
- navigation and subresource referrer policy wiring;
- browser single-hop redirect orchestration;
- Fetch request/response types and no-CORS opaque responses;
- redirect modes and opaque-redirect behavior;
- cross-origin simple Fetch and credentialed CORS;
- OPTIONS preflight and redirect-target preflight;
- session-scoped CORS preflight permissions and invalidation;
- request/response Headers guards and forbidden header rules;
- body cloning, disturbance, BodyInit, null-body status and HEAD behavior;
- Fetch Subresource Integrity verification;
- CORS safelist, Range, ByteString-length, token-list, HTTP-OWS and Max-Age parsing boundaries.

The lane deliberately does **not** claim a complete Fetch implementation, full streaming body semantics, a browser-global standards-complete network partition key, or complete web-platform URL parsing.

### Lane B — Integrity-Policy / Reporting API

Retain the newest cumulative Reporting/Integrity tail rather than every ancestor PR. The lane covers Integrity-Policy parsing/enforcement/report generation, Reporting-Endpoints resolution, delivery scheduling, retries, Retry-After handling, 410 endpoint removal, Structured Fields parsing and related lifecycle tests.

This lane must be reconciled against Lane A before merge because both inherit and modify overlapping Fetch/session code.

### Lane C — HTTP cache primitives

Retain the latest cumulative cache tail rather than individual cache-model PRs. This lane currently models request cache modes, Fetch Metadata destination/request primitives, response cache policy, Vary matching, current-age arithmetic and revalidation validators. It is not yet a complete wired browser HTTP cache.

### Lane D — CORP / COEP

Retain the latest cumulative COEP/CORP tail. This lane contains the transport-neutral CORP policy, COEP-aware internal checks, response-header parsing and document policy-container work. Concrete loader integration remains subject to Stage 1 review.

### Small independent lanes

Small features that are not safely represented by the umbrella lanes (for example form `rel=noreferrer` and response MIME policy primitives) should remain separate only when they are not already superseded by a retained tail.

## Pull-request debt policy

An older PR should be closed when one of the following is true:

- its exact changes are already contained in a retained cumulative tail;
- a later PR explicitly reimplemented the same fix on the current stack;
- it is an abandoned sibling whose behavior is represented by a newer implementation;
- keeping it open provides no independent review or integration value.

Closing a stacked ancestor is bookkeeping only; its branch may remain available as Git history for descendants.

## Integration gates

A retained umbrella is eligible to move forward only when:

- it is rebased/retargeted to the intended integration base;
- the exact head passes the full locked Cargo test workflow;
- no temporary patch/workflow helpers remain;
- standards/security claims in the PR body have direct regression coverage;
- known inherited warnings are documented and no new warnings are introduced unintentionally;
- overlapping lanes have an explicit integration order;
- duplicate implementations are removed rather than kept as parallel production paths;
- the final integration candidate is reviewable as a coherent subsystem rather than an unbounded feature stream.

## Current priority

1. Freeze new feature generation.
2. Make PR #267 the Lane A umbrella against `main`.
3. Close Lane A ancestors and obvious superseded siblings.
4. Collapse HTTP cache and COEP/CORP stacks to their tails.
5. Collapse Integrity/Reporting to one retained tail, then reconcile it against Lane A.
6. Run exact-head CI after every retarget/consolidation step.
7. Merge only after the retained lanes are conflict-resolved and their guarantees are documented.
