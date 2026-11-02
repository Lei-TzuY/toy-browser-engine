# M2-R1 first verified fix — color-only background shorthand

Original v0.1.0 Linux release baseline remains frozen: 18 PASS / 29 executed.

Modified engine from main 54283dec2a22e659a90a62ae97d9274d9facfcb1: 25 PASS / 29 executed; new PASS cases C02-C08 (7 improvements), no previously passing case regressed. The post-fix binary is an untagged local debug build, NOT the published release asset. Diff SHA-256: 9a49513924cd89a083eb6237b377ac993cea817b57805abe7b0fedc2c6b80585.

This **isolated fix** expands CSS declarations of a simple solid-color background shorthand to background-color during the existing cascade. The style map then permits painting to observe the color; declarations remain ordered for specificity, inline styles and importance. It is **NOT a complete CSS background shorthand implementation**.

Targeted paint test: one new multi-case regression covers color shorthand/longhand order and !important. Linux WSL2 full Rust suite: 1,894/1,894 PASS (194 test groups), 0 failed/ignored. Existing inherited compiler warnings were present.

Evidence:
- Baseline: baseline.json (published binary SHA-256 a5f5ed565cd8137d15f62793544a8b988c1cc3266f02e1046093545130ef020f).
- After: after-background.json and after-background-captures.tar.gz; all 29 input hashes match the original baseline.
- Runner: m2_probe.py (baseline) and run_after_background.py (optional alternate executable/output directory).

Four cases still FAIL: C01 root/html background paint (separate root/viewport behavior), H02 HTML serialization escaping (not proven parsing error), J12 async-function execution unsupported, J13 Array.reduce returns undefined. These require independent root cause and expected-output verification before further fixes.

Safety: synthetic local files only; not real website compatibility, Windows-native GUI, HTTPS, or untrusted browsing. Independent Reviewer C and exact-head CI still required. No user-facing release claim.
