//! Safe host adapter. Raw Windows APIs live in mayhem-windows-sandbox only.
use super::*;
use mayhem_windows_sandbox::{DecoderChild, DecoderLauncher, DecoderMode};
use std::{io, os::windows::process::ExitStatusExt, process::ExitStatus};

pub(super) type ChildStdin = tokio::fs::File;
pub(super) type ChildStdout = tokio::fs::File;
pub(super) type Launcher = DecoderLauncher;

pub(super) struct Child {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    child: DecoderChild,
}
pub(super) fn spawn(launcher: &Launcher, mode: BundledMode) -> Result<Child> {
    let mode = match mode {
        BundledMode::Decoder => DecoderMode::Decoder,
        BundledMode::Tokenizer => DecoderMode::Tokenizer,
    };
    let mut child = launcher.spawn(mode).map_err(|_| Error::Start)?;
    let mut input = tokio::fs::File::from_std(child.stdin.take().ok_or(Error::Start)?);
    let mut output = tokio::fs::File::from_std(child.stdout.take().ok_or(Error::Start)?);
    input.set_max_buf_size(CHUNK_BYTES);
    output.set_max_buf_size(CHUNK_BYTES);
    Ok(Child {
        stdin: Some(input),
        stdout: Some(output),
        child,
    })
}
impl Child {
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.child
            .kill()
            .map_err(|_| io::Error::other("contained decoder termination failed"))
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        // One bounded supervisor per acquired process slot. Independent of token,
        // receipt and history counts; cancellation drops this wait safely.
        loop {
            match self
                .child
                .try_wait()
                .map_err(|_| io::Error::other("contained decoder wait failed"))?
            {
                Some(code) => return Ok(ExitStatus::from_raw(code)),
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }
}
