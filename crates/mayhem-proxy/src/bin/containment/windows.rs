//! Windows restriction is established by the broker before resume. Verify the
//! actual token/job/handles and install irreversible mitigations before IPC.
pub(super) fn enter() -> Result<(), ()> {
    mayhem_windows_sandbox::verify_decoder_process().map_err(|_| ())
}
