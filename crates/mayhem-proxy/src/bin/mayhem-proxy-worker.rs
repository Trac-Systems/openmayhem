fn main() {
    // Dedicated stdio executable. No inherited configuration discovery or logs.
    let ok = std::env::args_os().skip(1).collect::<Vec<_>>() == ["--stdio-v1"]
        && mayhem_proxy::worker::disable_core_dumps().is_ok()
        && mayhem_proxy::worker::run(std::io::stdin().lock(), std::io::stdout().lock()).is_ok();
    if !ok {
        std::process::exit(2);
    }
}
