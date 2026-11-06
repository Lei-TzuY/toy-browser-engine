#!/usr/bin/env bash
# Independent Reviewer C supplement: run on an existing checkout without changing production source.
# At c9dd2ca expect 3 PASS / 2 FAIL; at f330aea expect 5 PASS / 0 FAIL.
set -euo pipefail
review_repo=$(realpath "${1:?usage: reproduce-reviewer-c-probes.sh SOURCE_CHECKOUT NEW_OUTPUT_DIR}")
review_out=$(realpath -m "${2:?usage: reproduce-reviewer-c-probes.sh SOURCE_CHECKOUT NEW_OUTPUT_DIR}")
review_here=$(cd -- "$(dirname -- "$0")" && pwd)
review_probe="$review_repo/tests/reviewer_c_select_event_probe_v2.rs"
test ! -e "$review_probe"
test ! -e "$review_out/select-event-probe.log"
mkdir -p "$review_out"
git -C "$review_repo" rev-parse HEAD > "$review_out/source-head.txt"
git -C "$review_repo" status --porcelain > "$review_out/source-status-before.txt"
sha256sum "$review_here/reviewer_c_select_event_probe_v2.rs" > "$review_out/probe-source-sha256.txt"
cp "$review_here/reviewer_c_select_event_probe_v2.rs" "$review_probe"
trap 'rm -f "$review_probe"' EXIT
cd "$review_repo"
CARGO_BUILD_JOBS=2 cargo test --locked --test reviewer_c_select_event_probe_v2 -- --nocapture 2>&1 | tee "$review_out/select-event-probe.log"
