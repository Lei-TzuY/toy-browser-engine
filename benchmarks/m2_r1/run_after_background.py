from pathlib import Path
import json
import sys
import m2_probe
if len(sys.argv)!=3:
    raise SystemExit("Usage: python run_after_background.py /path/to/patched/browser_engine /path/to/after-output")
m2_probe.BIN=Path(sys.argv[1]).resolve()
m2_probe.OUT=Path(sys.argv[2]).resolve()
m2_probe.PAGES=m2_probe.OUT/"pages"
m2_probe.CAP=m2_probe.OUT/"captures"
m2_probe.run()
path=m2_probe.OUT/"baseline.json"
report=json.loads(path.read_text(encoding="utf-8"))
report["run_label"]="post-fix candidate binary, not published v0.1.0"
report["release_tag"]=None
report["release_sha"]=None
path.write_text(json.dumps(report,indent=2,ensure_ascii=False)+"\n",encoding="utf-8")
