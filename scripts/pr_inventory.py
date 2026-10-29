#!/usr/bin/env python3
r"""Read-only Git analysis of a paginated GitHub open-PR snapshot.

Fetch a snapshot with `gh api --paginate --slurp
repos/Lei-TzuY/toy-browser-engine/pulls?state=open\&per_page=100`.
No branches, refs, PRs, or working files are changed by this script.
"""
import argparse
import collections
import datetime
import hashlib
import json
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--main", required=True, help="Exact main commit, never a moving ref")
    parser.add_argument("--main-ci-url", help="Optional verified CI receipt for that exact commit")
    parser.add_argument("--review-evidence", type=Path, help="Exact-head manual review and closeout receipts")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    def git(*words, check=True):
        return subprocess.run(
            ["git", "-C", str(args.repo), *words], check=check,
            capture_output=True, text=True, encoding="utf-8",
        )

    def ancestor(older, newer):
        result = git("merge-base", "--is-ancestor", older, newer, check=False)
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr)
        return result.returncode == 0

    snapshot = json.loads(args.snapshot.read_text(encoding="utf-8"))
    if isinstance(snapshot, dict):
        snapshot = snapshot["prs"]
    if snapshot and isinstance(snapshot[0], list):
        snapshot = [pr for page in snapshot for pr in page]
    assert len({pr["number"] for pr in snapshot}) == len(snapshot), "duplicate PRs"
    main_sha = git("rev-parse", args.main + "^{commit}").stdout.strip()
    reviews = json.loads(args.review_evidence.read_text(encoding="utf-8")) if args.review_evidence else {}
    if reviews:
        assert reviews["main_commit"] == main_sha, "review evidence belongs to another main"
    branch_owners = {pr["head"]["ref"]: pr["number"] for pr in snapshot}
    refs = {main_sha}
    for pr in snapshot:
        refs.update((pr["head"]["sha"], pr["base"]["sha"]))
    graph = {}
    for line in git("rev-list", "--parents", *sorted(refs)).stdout.splitlines():
        commit, *parents = line.split()
        graph[commit] = parents
    head_numbers = collections.defaultdict(list)
    for pr in snapshot:
        head_numbers[pr["head"]["sha"]].append(pr["number"])
    descendants = collections.defaultdict(set)
    for pr in snapshot:
        pending, seen = [pr["head"]["sha"]], set()
        while pending:
            commit = pending.pop()
            if commit in seen:
                continue
            seen.add(commit)
            for number in head_numbers[commit]:
                if number != pr["number"]:
                    descendants[number].add(pr["number"])
            pending.extend(graph.get(commit, ()))

    records = []
    for pr in sorted(snapshot, key=lambda item: item["number"]):
        number, head, base = pr["number"], pr["head"], pr["base"]
        head_sha, base_sha = head["sha"], base["sha"]
        base_ref = git("rev-parse", "refs/remotes/origin/" + base["ref"], check=False)
        current_base = base_ref.stdout.strip() if base_ref.returncode == 0 else None
        head_ref = git("rev-parse", "refs/remotes/origin/" + head["ref"], check=False)
        current_head = head_ref.stdout.strip() if head_ref.returncode == 0 else None
        reachable = ancestor(head_sha, main_sha)
        unique = git("rev-list", main_sha + ".." + head_sha).stdout.splitlines()
        topic = git("rev-list", base_sha + ".." + head_sha).stdout.splitlines()
        patches = []
        if not reachable:
            for line in git("cherry", main_sha, head_sha, base_sha).stdout.splitlines():
                sign, commit = line.split(maxsplit=1)
                patches.append({"commit": commit, "equivalent_in_main_history": sign == "-"})

        def merge_evidence(target):
            if ancestor(head_sha, target):
                return {"conflicts": False, "reason": "head_already_reachable_from_target"}
            if ancestor(target, head_sha):
                return {"conflicts": False, "reason": "target_is_ancestor_of_head"}
            result = git("merge-tree", "--write-tree", "--name-only", target, head_sha, check=False)
            if result.returncode not in (0, 1):
                return {"conflicts": None, "reason": result.stderr.strip()}
            return {
                "conflicts": result.returncode == 1,
                "reason": "git_merge_tree_without_checkout",
                "result_tree": result.stdout.splitlines()[0],
                "diagnostics": [line for line in result.stdout.splitlines() if line.startswith("CONFLICT")],
                "receipt_sha256": hashlib.sha256(result.stdout.encode()).hexdigest(),
            }

        against_main = merge_evidence(main_sha)
        against_base = merge_evidence(current_base) if current_base else {
            "conflicts": None, "reason": "current base ref unavailable"}
        if reachable:
            category, confidence = "already_integrated", "high"
            superseded, useful = False, False
            rationale = "Every commit at this exact PR HEAD is reachable from the tested main commit."
        elif patches and all(patch["equivalent_in_main_history"] for patch in patches):
            category, confidence = "superseded", "medium"
            superseded, useful = True, None
            rationale = "Every topic patch has a patch-id equivalent in main history; final behavior still needs review."
        elif against_main["conflicts"]:
            category, confidence = "conflicting_blocked", "high"
            superseded, useful = None, None
            rationale = "Git merge-tree finds a conflict against exact main; value is not inferred from the conflict."
        else:
            category, confidence = "insufficient_evidence", "low"
            superseded, useful = None, None
            rationale = "Unique history is not proof of useful behavior; retain for source/behavior review."

        dependents = sorted(other["number"] for other in snapshot
                            if other["base"]["ref"] == head["ref"])
        patch_base = git("merge-base", base_sha, head_sha).stdout.strip()
        patch = git("diff", "--binary", patch_base, head_sha).stdout
        records.append({
            "number": number, "title": pr["title"],
            "url": pr.get("url", pr.get("html_url")),
            "base": {"ref": base["ref"], "sha": base_sha, "current_ref_sha": current_base},
            "head": {"ref": head["ref"], "sha": head_sha, "current_ref_sha": current_head},
            "head_reachable_from_main": reachable,
            "unique_commits_against_main": unique,
            "topic_commits_against_snapshot_base": topic,
            "topic_patch_sha256": hashlib.sha256(patch.encode()).hexdigest(),
            "changed_files": git("diff", "--name-only", patch_base, head_sha).stdout.splitlines(),
            "patch_equivalence": patches,
            "patch_or_behavior_superseded": superseded,
            "independent_still_useful_changes": useful,
            "behavior_evidence": "Reachability/patch evidence only; no claim that every historical behavior remains enabled.",
            "explicit_base_dependents": dependents,
            "head_commit_descendants": sorted(descendants[number]),
            "base_pr": branch_owners.get(base["ref"]),
            "merge_against_main": against_main,
            "merge_against_current_base": against_base,
            "classification": category, "confidence": confidence,
            "recommended_disposition": "eligible_for_bounded_closeout" if reachable and not dependents else "retain",
            "rationale": rationale,
            "verification": {
                "main_commit": main_sha,
                "main_ci": args.main_ci_url,
                "individual_head_tests": "not rerun; main CI does not independently validate an unmerged head",
            },
            "closeout": {"closed_in_this_batch": False, "branch_preserved": True},
        })
        record = records[-1]
        for review in reviews.get("reviews", []):
            if review["number"] == number:
                assert review["head_sha"] == head_sha
                for field in ("classification", "confidence", "rationale", "independent_still_useful_changes"):
                    record[field] = review[field]
                record["manual_review_evidence"] = review["evidence"]
        for closed in reviews.get("closeouts", []):
            if closed["number"] == number:
                assert closed["head"] == head_sha and reachable and not dependents
                record["closeout"] = closed
                record["recommended_disposition"] = "closed_already_integrated_branch_preserved"
    counts = {category: 0 for category in (
        "already_integrated", "superseded", "still_independently_valuable",
        "conflicting_blocked", "insufficient_evidence")}
    counts.update(collections.Counter(pr["classification"] for pr in records))
    output = {
        "schema_version": 1,
        "captured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "main_commit": main_sha,
        "snapshot_open_pr_count": len(records),
        "classification_counts": counts,
        "policy": "Never close by title, age or CI alone. Uncertainty stays open. Preserve all source branches. A bounded first batch may close only reachable heads without explicit base dependents.",
        "prs": records,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"count": len(records), "classifications": output["classification_counts"],
                      "eligible_leaf_prs": [pr["number"] for pr in records if pr["recommended_disposition"].startswith("eligible")]}, indent=2))


if __name__ == "__main__":
    main()
