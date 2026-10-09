//! OS restrictions for the bundled decoder, installed before reading any IPC.
//! Keep platform FFI in this executable; the financial/protocol library continues
//! to forbid unsafe code. This is not an arbitrary executable plugin launcher.

#[cfg(target_os = "macos")]
mod macos;

pub fn enter() -> Result<(), ()> {
    #[cfg(target_os = "macos")]
    return macos::enter();
    // Other platforms retain the existing process restrictions until their
    // separately verified containment implementations are integrated. Do not
    // represent this as cross-platform filesystem/network isolation.
    #[cfg(not(target_os = "macos"))]
    Ok(())
}
