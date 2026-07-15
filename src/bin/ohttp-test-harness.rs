//! Prints harness URLs as one JSON line on stdout, then parks until killed.
//!
//! Used by the JS e2e test. Start with:
//! `cargo run --features harness --bin ohttp-test-harness`

#[path = "../../tests/harness/mod.rs"]
mod harness;

fn main() {
    let h = harness::TestHarness::start();
    // Prefix so consumers can ignore other crates' stdout noise (e.g. ohttp-relay).
    println!(
        "OHTTP_HARNESS\t{{\"relay_url\":\"{}\",\"gateway_url\":\"{}\",\"target_url\":\"{}\",\"connect_proxy_url\":\"{}\"}}",
        h.relay_url(),
        h.gateway_url(),
        h.target_url(),
        h.connect_proxy_url(),
    );
    // Hold servers up until the parent process kills us.
    loop {
        std::thread::park();
    }
}
