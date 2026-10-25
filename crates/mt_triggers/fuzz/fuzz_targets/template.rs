#![no_main]
use libfuzzer_sys::fuzz_target;

// The template parser, the expander and the sanitiser must never panic and the sanitiser's
// output must stay bounded and free of control characters (SPEC §10.11.2).
fuzz_target!(|data: &str| {
    let _ = mt_triggers::template::parse(data);
    let out = mt_triggers::template::expand(data, |_| Some(data.to_string()));
    let s = mt_triggers::template::sanitize(data);
    assert!(s.chars().count() <= mt_triggers::template::VALUE_MAX);
    assert!(!s.chars().any(char::is_control));
    let _ = out;
});
