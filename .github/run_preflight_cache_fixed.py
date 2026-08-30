from pathlib import Path
import runpy

runpy.run_path('.github/apply_preflight_cache.py', run_name='__main__')

test = Path('tests/fetch_cors_preflight_cache.rs')
test.write_text(test.read_text().replace('\\n', '\n'))

path = Path('src/script/fetch_api.rs')
text = path.read_text()
old = '''        let wildcard = !cors.credentialed && allowed.iter().any(|header| header == "*");
        for requested in &cors.requested_headers {
            if !wildcard
                && !allowed
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(requested))
            {
'''
new = '''        let wildcard = !cors.credentialed && allowed.iter().any(|header| header == "*");
        for requested in &cors.requested_headers {
            let wildcard_allows = wildcard && !is_cors_non_wildcard_request_header_name(requested);
            if !wildcard_allows
                && !allowed
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(requested))
            {
'''
assert old in text
path.write_text(text.replace(old, new, 1))
