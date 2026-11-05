# M2-R1 invalid background validation correction

Independent review identified four invalid-declaration regressions in candidate b3ca11ac0cee57fcd1c5be0925af6e961f60c673. The original three fixtures were reproduced unchanged in a fresh build: invalid background:nonsense erased valid background-color or background-image, including when the invalid declaration was important. A separate fourth unchanged fixture reproduced background:red nonsense incorrectly overriding an earlier blue background-color.

The correction validates the supported value before resetting longhands or replacing the stored shorthand fallback. A background-only parser check also rejects extra tokens after the supported value while retaining comments and exact !important syntax. Existing solid colors, variables, linear/radial/conic gradients and none retain their supported behavior. This adds color-only shorthand support while preserving the engine's existing gradient/none subset; it does not implement full CSS background grammar, CSS-wide keyword semantics or URL-backed backgrounds.

## Actual local results

| Fixture group | Before b3ca11ac | Corrected tree |
|---|---|---|
| Unchanged independent review fixtures | 23/26 | 26/26 |
| Adjacent invalid trailing-token fixture | red, expected blue (FAIL) | blue (PASS) |
| Frozen 29-case harness metric | Candidate 25/29 | 25/29; no prior PASS regression |
| Full Rust tests | Prior candidate 1,894 PASS | 1,897 PASS; 0 FAIL; 0 ignored; 194 groups |

The original baseline and both prior capture archives are unchanged. All 29 HTML fixture hashes still match. Remaining frozen harness failures are C01, H02, J12 and J13. The 18/29 release to 25/29 candidate change is a frozen harness metric: some DOM substring checks can match literal script source (notably J02/J07), so those checks do not establish live DOM behavior. The seven C02-C08 paint improvements are independently verified pixels.

## Evidence and reproduction

invalid-background-cascade.json records exact commands, expected and actual pixel (5,5) values, input/capture hashes, binary hashes, final source fingerprints and full test totals. invalid-background-captures.tar.gz contains the unchanged 27 HTML fixtures, before/after raw PPMs and inspect output, final 29-case outputs and actual build/test logs. No executable is included.

Before binary SHA-256: 035d713f3c0a0a33a2ee978bc84002ac8b93ed41741487e1dfa21f065b904edc

Corrected binary SHA-256: adbdd7ed6ba49975c5333fea6cff18fad68cdf55553aac080589793582f2c24c

Tested source patch SHA-256: 491bb0ded47ed2decb9e81b5dd2de484af95bb37ff1ff29592894bd7264783a4

Archive SHA-256: bbbcc2f50660acd8fc27b40206a2f79010ba8f65064b7fe5f63d2997fafabd53

Run `cargo test --locked --lib background` for focused parser/cascade/paint coverage, or `cargo test --locked` for the full suite. To replay a captured fixture, extract the archive and invoke the desired binary with `--quiet --inspect --size 120x80 --screenshot capture.ppm fixtures/CASE.html`. The receipt contains each original argument vector. Check RGB pixel (5,5) in the P6 output against the recorded expected value. The archived check-cascade.py is the actual provenance runner and retains its original host paths.

All measurements here use trusted local files in WSL CLI mode. They do not establish GUI, IME, real website compatibility or an arbitrary-content security boundary. The receipts were produced before the corrective commit; the source fingerprints identify the tested implementation. Independent corrected-HEAD review and CI remain the merge gate.
