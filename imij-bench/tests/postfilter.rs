// The timing harness has harness=false; expose the same source's correctness
// tests through ordinary `cargo test --test postfilter` as well.
#[allow(dead_code)]
#[path = "../src/postfilter.rs"]
mod postfilter;
