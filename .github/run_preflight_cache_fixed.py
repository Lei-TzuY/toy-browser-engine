from pathlib import Path
import runpy

runpy.run_path('.github/apply_preflight_cache.py', run_name='__main__')
test = Path('tests/fetch_cors_preflight_cache.rs')
test.write_text(test.read_text().replace('\\n', '\n'))
