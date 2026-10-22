#![no_main]
//! Parsing and analysis must never panic, and diagnostics must carry usable spans.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(src) = std::str::from_utf8(data) {
        let t = mt_snippets::Template::compile(src);
        for d in t.diagnostics() {
            let _ = d.line_col(src);
            let _ = d.span.slice(src);
        }
    }
});
