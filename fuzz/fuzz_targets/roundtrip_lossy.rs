#![no_main]

use libfuzzer_sys::fuzz_target;

mod imp {
    include!("roundtrip_lossy_core.rs");
}

fuzz_target!(|data: &[u8]| {
    imp::run(data);
});
