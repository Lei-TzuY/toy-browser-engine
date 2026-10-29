# M1 stabilization — v0.1.0 Research Preview

Master tracker: [#286](https://github.com/Lei-TzuY/toy-browser-engine/issues/286).
Baseline: `181f4a10287701e3615c4406f7acd24c0efe5ed4`.

## Completed integrations

The former instructions to merge/consolidate these umbrellas are historical:

| Lane | Merged PR | Merge commit |
| --- | --- | --- |
| Fetch/navigation/session policy | [#267](https://github.com/Lei-TzuY/toy-browser-engine/pull/267) | `6bf1ad9c3ffe609c3da6b4bd521441390fff70aa` |
| Integrity-Policy/Reporting | [#238](https://github.com/Lei-TzuY/toy-browser-engine/pull/238) | `3d22a5e76ac7c1e8e278f98e1219d9991747f83c` |
| HTTP cache primitives | [#185](https://github.com/Lei-TzuY/toy-browser-engine/pull/185) | `2b06d340bd0d6d1e731ed130375ace2cb0004b1b` |
| COEP/CORP components | [#194](https://github.com/Lei-TzuY/toy-browser-engine/pull/194) | `67120316c34113ea5cb4eaed057e081fe484cef2` |

PRs #283, #284 and #285 are merged too: tabs/native provenance, forbidden-header
socket tests and redirect method/body socket tests. Do not redo these integrations.
Merged policy/API components do not establish complete default-Browser wiring,
standards compliance or sandbox isolation.

## Freeze and gates

M1 permits focused correctness/portability fixes, regression tests, truthful docs,
bounded CI, package/license work and reproducible native smoke. Freeze unrelated
Web APIs, DOM/CSS/rendering expansions, profile persistence, installers, signing
infrastructure and auto-updaters.

Use one coherent release-readiness PR. Preserve main history, source branches,
unrelated repositories/services/workloads and host security controls. Compare
format/Clippy with exact main rather than reformatting unrelated files.

[Release readiness](docs/release-readiness.md) and candidate receipts define the
gates: Linux and native Windows build/test/GUI checks, extracted package/hash and
license verification, exact-head CI and a separate Reviewer C. Self-review is not
independent approval. Missing essential evidence means RELEASE_HOLD. Verify the
real GitHub Release URL/assets before reporting publication.

## Historical PR debt

The complete initial snapshot has 185 open PRs; that is not a missing-feature
count. [Machine-readable inventory](docs/pr-inventory.json) records base/head,
main reachability, patch/merge checks, dependencies and confidence.
[scripts/pr_inventory.py](scripts/pr_inventory.py) reproduces the read-only Git
analysis; exact-head manual receipts are separate.

Only a bounded batch of three integrated leaf PRs (#176, #196, #233) is closed;
source branches remain. Conflicted/uncertain proposals and integrated parents
with open base dependents remain open. Closing an ancestor never substitutes
for checking a descendant's unique work.

Legacy issue #268 predates the merged umbrellas; #286 is now the M1 tracker.
This update does not claim every old acceptance condition or feature proposal
is complete.
