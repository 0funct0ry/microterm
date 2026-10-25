#![no_main]
use libfuzzer_sys::fuzz_target;
use mt_term::private_osc::{parse, PrivateGate, PrivateState};
use mt_term::{Emulator, VtConfig};

// The private OSC dispatcher (SPEC §6.4) must never panic, whatever the bytes, with the master
// switch on or off, and the parser must accept only payloads of the block.
fuzz_target!(|data: &[u8]| {
    if let Some(cmd) = parse(data) {
        assert!(data.starts_with(b"77"));
        let mut st = PrivateState::default();
        let mut menus = Default::default();
        let _ = st.handle(&PrivateGate::default(), &mut menus, cmd, true, 0);
    }
    for enabled in [false, true] {
        let mut e = Emulator::new(40, 5, 100);
        e.set_vt_config(VtConfig {
            private: PrivateGate {
                enabled,
                ..Default::default()
            },
            ..Default::default()
        });
        let mut framed = b"\x1b]".to_vec();
        framed.extend_from_slice(data);
        framed.push(7);
        e.advance(&framed);
        e.advance(data);
        assert!(e.status_segments().len() <= mt_term::private_osc::STATUS_SLOTS);
    }
});
