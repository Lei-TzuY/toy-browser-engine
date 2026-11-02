# M2-R1 controlled-fixture baseline — exploratory, not reviewed

This repository addition contains a **measured, not invented, 29-case baseline** using the actual v0.1.0 Linux release binary with trusted local HTML/JavaScript/PPM inputs. It is NOT evidence of compatibility with arbitrary real websites or a production security claim.

## Frozen measurement
- v0.1.0 tag SHA: a065c9d3f37ac573e937b4606a4ee3007016b0b2
- Linux release TAR.GZ SHA-256: 5539eeb1669b8c64541ecb0351897a52e6bc7227e70d83f437be4dbd9f01caca
- Extracted binary SHA-256: a5f5ed565cd8137d15f62793544a8b988c1cc3266f02e1046093545130ef020f
- Environment: authorized DESKTOP-FLR11EU, WSL Ubuntu 26.04, headless CLI only, trusted local fixtures, no GPU
- 29 tests executed: **18 PASS, 11 FAIL**
- Area counts: HTML 4/5, CSS/paint 0/8, images 1/1, JS/DOM 12/13, JS/async 1/2

The script m2_probe.py defines and executes all 29 cases. baseline.json contains per-case execution arguments, fixture SHA, exact expected/observed checks, stdout/stderr excerpts and screenshot hashes. captures-fixtures.tar.gz contains all tested local fixtures and the 29 actual PPM screenshots.

## Reproduction
From benchmarks/m2_r1 on a controlled Linux x86-64 development environment:

    curl -fL -o preview.tar.gz https://github.com/Lei-TzuY/toy-browser-engine/releases/download/v0.1.0/toy-browser-engine-v0.1.0-research-preview-x86_64-unknown-linux-gnu.tar.gz
    sha256sum preview.tar.gz
    mkdir -p release
    tar -xzf preview.tar.gz -C release
    python3 m2_probe.py

Verify the downloaded SHA-256 before extracting. The script writes to a local baseline-v0.1.0 directory; the original baseline.json contains absolute original-host paths, so reproduction on another machine changes paths and stdout SHA values. Do not overwrite the archived original results with post-fix runs.

## Caveats and root cause hypotheses
- All eight CSS screenshot checks produced all-white 120x80 PPMs. A separate diagnostic showed white even with an inline red background and visible text. **Do not infer eight independent CSS bugs**. A shared rendering defect or test assumption needs independent investigation.
- H02 expects correctly escaped HTML serialization text but the dump showed unescaped ampersand and less-than text. This is not proof that HTML entity parsing failed.
- J12 remained PENDING (async functions), and J13 output undefined (Array.reduce). Confirm respective causes before engineering changes.
- Not covered: native Windows, live GUI, navigation interactions, HTTPS, IME, untrusted sites, sandbox safety, actual internet-site compatibility.
- This is a **partial M2 baseline**, NOT the full M2 milestone or a release candidate.

Gate: independent review, richer GUI/loopback controlled scenarios, bounded verified fixes with before/after, and exact-head CI before merge. Preserve main and published release.
