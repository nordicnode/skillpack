// A deliberately-broken CLI: `--help` exits non-zero instead of printing
// usage. `skillpack verify`'s `invocation.help_present` check fails
// critically on this, so `init`'s pre-commit gate hits a critical failure.
// Used by the integration test that asserts declining-without-writing exits
// code `INIT_FIXABLE` (2), not `INIT_ABORTED` (1).
use std::process::exit;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--help" {
        // non-zero on --help -> verify critical
        exit(2);
    }
    println!("bad-help");
}
