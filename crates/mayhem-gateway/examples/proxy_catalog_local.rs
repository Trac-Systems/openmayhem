//! Opt-in LOCAL TEST catalog; Unix containment is required.
#[cfg(unix)]
#[path = "proxy_catalog_local/unix.rs"]
mod unix;

#[cfg(unix)]
fn main() {
    unix::main();
}

#[cfg(not(unix))]
fn main() {
    eprintln!("Local test catalog is unsupported on this platform: Unix private-file and process containment is required.");
    std::process::exit(2);
}
