//! OS restrictions for the bundled decoder, installed before reading any IPC.
//! Keep platform FFI in this executable; the financial/protocol library continues
//! to forbid unsafe code. This is not an arbitrary executable plugin launcher.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
mod windows;

#[cfg(all(
    target_os = "linux",
    target_pointer_width = "64",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linux;

pub fn enter() -> Result<(), ()> {
    #[cfg(target_os = "macos")]
    return macos::enter();
    #[cfg(windows)]
    return windows::enter();
    #[cfg(all(
        target_os = "linux",
        target_pointer_width = "64",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    return linux::enter();
    #[cfg(all(
        target_os = "linux",
        not(all(
            target_pointer_width = "64",
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
    ))]
    return Err(());
    // Other platforms retain the existing process restrictions until their
    // separately verified containment implementations are integrated. Do not
    // represent this as cross-platform filesystem/network isolation.
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Ok(())
}
