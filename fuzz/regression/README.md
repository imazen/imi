# Fuzz regression inputs

Minimized inputs (`cargo fuzz tmin` output, under 8 KB each) that once crashed or failed a fuzz target and
pass on `main` since their fix. `imij/tests/fuzz_regression.rs` replays every file here through every
target's logic on stable Rust. Working corpora and raw crash artifacts are kept outside the repository.
