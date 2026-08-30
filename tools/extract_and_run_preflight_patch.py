from pathlib import Path

source = Path('.github/workflows/agent-fetch-cors-preflight-patch.yml')
lines = source.read_text().splitlines()
start = None
end = None
for index, line in enumerate(lines):
    if line.strip() == 'run: |' and index > 0 and 'Apply CORS preflight implementation and tests' in lines[index - 2]:
        start = index + 1
        continue
    if start is not None and line.strip() == '- name: Commit CORS preflight implementation':
        end = index
        break
if start is None or end is None:
    raise SystemExit('could not locate embedded preflight patch body')

code_lines = []
for line in lines[start:end]:
    if line.startswith('          '):
        code_lines.append(line[10:])
    elif not line.strip():
        code_lines.append('')
    else:
        raise SystemExit(f'unexpected patch indentation: {line!r}')

code = '\n'.join(code_lines) + '\n'
exec(compile(code, str(source), 'exec'), {'__name__': '__main__'})

Path('.github/workflows/agent-run-preflight-patch.yml').unlink()
Path('tools/extract_and_run_preflight_patch.py').unlink()
