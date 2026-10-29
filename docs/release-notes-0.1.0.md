# v0.1.0 Research Preview

**Experimental, unsigned research software for controlled/trusted content.**
Not a production-safe general browser; no commercial-browser compatibility
certification or verified process sandbox.

This preview packages the existing independent Rust engine: HTML/CSS/layout/paint,
an inspectable JavaScript interpreter and DOM/event loop, verified TLS/session
networking, experimental multi-tab chrome, and reproducible fixtures.

M1 consolidates documentation, historical-PR evidence, Ubuntu/Windows CI and native
smoke/package receipts. It does not redo merged integrations or add unrelated Web
APIs, profile persistence, installers, signing infrastructure or auto-updates.

Assets: Linux x86-64 TAR.GZ, Windows x86-64 ZIP, SHA256SUMS, build/source identity,
dependency notices and candidate evidence. Linux's OS/glibc floor is its measured
build environment; other distributions should build from source. Windows keyboard
smoke is scoped to US English; no working CJK IME composition workflow is promised.
The Windows EXE requires Microsoft's Visual C++ v14 x64 Redistributable
(`VCRUNTIME140.dll`), separate from the OS UCRT. Follow the official prerequisite
links in the build/install instructions; runtime installation may need administrator
approval. The runtime is not bundled, and stock Windows without it was not tested.

See [build/install instructions](build-and-run.md) and [limitations/gates](release-readiness.md).
GUI receipts are distinct from compilation/CLI CI. No byte-for-byte reproducible
build, arbitrary-site safety, signing or full-standards claim is made.

The final tracker/release receipt supplies the actual Release URL, exact SHA,
native counts, archive sizes/hashes and independent Reviewer C verdict. If gates
remain unresolved, these are candidate notes under RELEASE_HOLD, not publication.
