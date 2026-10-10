use super::*;
use crate::private_files::mutation::tests::Fixture;
use std::{fs, sync::Arc, thread};

#[test]
fn private_database_holds_exclusive_data_and_parent_handles_until_drop() {
    let fixture = Fixture::new();
    let path = fixture.0.join("database.redb");
    let file = PrivateDatabaseFile::open(&path, false).unwrap();
    file.write(0, b"retained database").unwrap();
    file.sync_data().unwrap();
    assert!(PrivateDatabaseFile::open(&path, false).is_err());
    assert!(PrivateDatabaseFile::open(&path, true).is_err());
    assert!(fs::read(&path).is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(&fixture.0, fixture.0.with_extension("moved")).is_err());
    drop(file);
    let reopened = PrivateDatabaseFile::open(&path, true).unwrap();
    let mut bytes = [0u8; 17];
    reopened.read(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"retained database");
}

#[test]
fn private_database_strict_reopen_does_not_create_empty_or_accept_hardlinks() {
    let fixture = Fixture::new();
    let path = fixture.0.join("database.redb");
    assert!(PrivateDatabaseFile::open(&path, true).is_err());
    assert!(!path.exists());
    assert!(PrivateDatabaseFile::open(&fixture.0.join(".mayhem-ntfs.lock"), false).is_err());
    assert!(!fixture.0.join(".mayhem-ntfs.lock").exists());
    drop(PrivateDatabaseFile::open(&path, false).unwrap());
    assert!(PrivateDatabaseFile::open(&path, true).is_err());
    let file = PrivateDatabaseFile::open(&path, false).unwrap();
    file.write(0, b"original").unwrap();
    file.sync_data().unwrap();
    drop(file);
    fs::hard_link(&path, fixture.0.join("alias.redb")).unwrap();
    assert!(PrivateDatabaseFile::open(&path, true).is_err());
    assert!(PrivateDatabaseFile::open(&path, false).is_err());
    assert_eq!(fs::read(path).unwrap(), b"original");
}

#[test]
fn private_database_positioned_io_terminates_at_eof_and_preserves_concurrent_pages() {
    let fixture = Fixture::new();
    let file = Arc::new(PrivateDatabaseFile::open(&fixture.0.join("pages.redb"), false).unwrap());
    file.set_len(4096).unwrap();
    assert_eq!(
        file.set_len(u64::MAX).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(file.len().unwrap(), 4096);
    let mut zeros = [1u8; 4096];
    file.read(0, &mut zeros).unwrap();
    assert_eq!(zeros, [0u8; 4096]);
    thread::scope(|scope| {
        for page in 0..8u64 {
            let file = &file;
            scope.spawn(move || {
                let bytes = [page as u8 + 1; 512];
                file.write(page * 512, &bytes).unwrap();
                let mut read = [0u8; 512];
                file.read(page * 512, &mut read).unwrap();
                assert_eq!(read, bytes);
            });
        }
    });
    file.sync_data().unwrap();
    assert_eq!(
        file.read(4096, &mut [0]).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(
        file.read(u64::MAX, &mut [0]).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        file.write(u64::MAX, &[0]).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}
