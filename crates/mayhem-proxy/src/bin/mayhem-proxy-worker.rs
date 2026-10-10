mod containment;
mod tokenizer_memory;

#[global_allocator]
static ALLOCATOR: tokenizer_memory::Allocator = tokenizer_memory::Allocator::new();

fn main() {
    // Dedicated stdio executable. No inherited configuration discovery or logs.
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let persistent = args == ["--tokenizer-stdio-v2"];
    let tokenizer = persistent || args == ["--tokenizer-stdio-v1"];
    if args == ["--stdio-v1"] {
        // One-way bypass: ordinary decoding adds no accounting RMW operations.
        ALLOCATOR.disable();
    }
    let ok = (tokenizer || args == ["--stdio-v1"])
        && mayhem_proxy::worker::disable_core_dumps().is_ok()
        && (!tokenizer
            || (ALLOCATOR
                .limit(mayhem_proxy::health::native::engine::HEAP_BYTES)
                .is_ok()
                && mayhem_proxy::health::native::engine::resource_limits(persistent).is_ok()))
        && containment::enter().is_ok()
        && if tokenizer {
            mayhem_proxy::health::native::engine::run(
                std::io::stdin().lock(),
                std::io::stdout().lock(),
                persistent,
            )
            .is_ok()
        } else {
            mayhem_proxy::worker::run(std::io::stdin().lock(), std::io::stdout().lock()).is_ok()
        };
    if !ok {
        std::process::exit(2);
    }
}
