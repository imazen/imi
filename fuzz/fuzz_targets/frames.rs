#![no_main]

use libfuzzer_sys::fuzz_target;

mod imp {
    include!("frames_core.rs");
}

fuzz_target!(|data: &[u8]| {
    imp::run(data);
});
