//! One fixed intent record and at most one staged image. The exclusive record
//! handle lives as long as the launcher and every child. Recovery never scans
//! history, follows links, deletes unknown names or repairs permissions.
use super::*;
use crate::private_files::PrivateDatabaseFile;

const RECORD: &str = ".decoder-image-v1";
const WORKER: &str = "mayhem-proxy-worker.exe";

pub(super) fn prepare(workdir: &Path) -> Result<(PrivateDatabaseFile, String)> {
    let entries = fs::read_dir(workdir)?
        .take(3)
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    if entries.len() > 2 || (!entries.is_empty() && !entries.iter().any(|name| name == RECORD)) {
        return Err(invalid());
    }
    // This existing protected primitive pins every ancestor and rejects aliases,
    // non-NTFS storage and non-private/hardlinked records. Sharing is zero: a
    // competing owner cannot clean up or launch into an active image directory.
    let record = PrivateDatabaseFile::open(&workdir.join(RECORD), false).map_err(|_| invalid())?;
    let length = record.len()?;
    if length > 64 {
        return Err(invalid());
    }
    if length == 64 {
        let mut bytes = [0u8; 64];
        record.read(0, &mut bytes)?;
        if !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Err(invalid());
        }
        let name = format!(
            "decoder-{}",
            std::str::from_utf8(&bytes).map_err(|_| invalid())?
        );
        if entries
            .iter()
            .any(|entry| entry != RECORD && entry != name.as_str())
        {
            return Err(invalid());
        }
        remove_previous(&workdir.join(name))?;
    } else if entries.iter().any(|entry| entry != RECORD) {
        // First write interrupted: no image may exist before a complete intent
        // is flushed. A damaged record never authorizes arbitrary cleanup.
        return Err(invalid());
    }
    let next = nonce()?;
    record.write(0, next.as_bytes())?;
    record.sync_data()?;
    Ok((record, next))
}

fn remove_previous(path: &Path) -> Result<()> {
    let directory = match open(
        path,
        FILE_READ_ATTRIBUTES | 0x20000 | DELETE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    ) {
        Ok(file) => file,
        Err(error) => {
            // Absence only; permissions, sharing, aliases and I/O errors fail.
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                _ => return Err(error),
            }
        }
    };
    let owner = current_user_sid()?;
    validate_acl(&directory, &owner, false)?;
    if !directory.metadata()?.is_dir() {
        return Err(invalid());
    }
    let children = fs::read_dir(path)?
        .take(2)
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    if children.len() > 1 || children.first().is_some_and(|name| name != WORKER) {
        return Err(invalid());
    }
    if !children.is_empty() {
        let file = open(
            &path.join(WORKER),
            FILE_READ_ATTRIBUTES | 0x20000 | DELETE,
            0,
        )?;
        validate_acl(&file, &owner, false)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
            || !file.metadata()?.is_file()
            || file.metadata()?.len() > 256 * 1024 * 1024
        {
            return Err(invalid());
        }
        delete_exact(&file)?;
        drop(file);
    }
    delete_exact(&directory)
}

fn delete_exact(file: &File) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            &disposition as *const _ as _,
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(last_error("decoder image recovery"));
    }
    Ok(())
}
