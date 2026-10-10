//! Real NTFS fixtures, compiled cross-target but requiring native execution.
use super::*;
use crate::private_files::mutation::tests::Fixture;
use std::fs;

fn leaf(v: &str) -> LeafName {
    LeafName::new(v).unwrap()
}
fn path(v: &[&str]) -> Vec<LeafName> {
    v.iter().map(|v| leaf(v)).collect()
}

#[test]
fn ntfs_directory_plan_rejects_invalid_trees_before_any_creation() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    let nested = path(&["state", "draft.json"]);
    let parent = path(&["state"]);
    let reserved = path(&[".mayhem-ntfs.lock"]);
    let deep = path(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
    let invalid = [
        vec![DirectoryEntry::File {
            path: &nested,
            bytes: b"missing parent",
        }],
        vec![
            DirectoryEntry::Directory { path: &parent },
            DirectoryEntry::Directory { path: &parent },
        ],
        vec![
            DirectoryEntry::File {
                path: &parent,
                bytes: b"file parent",
            },
            DirectoryEntry::File {
                path: &nested,
                bytes: b"x",
            },
        ],
        vec![DirectoryEntry::Directory { path: &reserved }],
        vec![DirectoryEntry::Directory { path: &deep }],
        vec![DirectoryEntry::Directory { path: &[] }],
    ];
    for entries in invalid {
        assert!(matches!(
            guard.stage_directory(leaf("invalid.next"), &entries, 64),
            Err(MutationError::Invalid)
        ));
        assert!(!f.0.join("invalid.next").exists());
    }
    let names = (0..129)
        .map(|n| path(&[&format!("dir{n}")]))
        .collect::<Vec<_>>();
    let entries = names
        .iter()
        .map(|p| DirectoryEntry::Directory { path: p })
        .collect::<Vec<_>>();
    assert!(matches!(
        guard.stage_directory(leaf("many.next"), &entries, 0),
        Err(MutationError::Invalid)
    ));
    let bytes = [DirectoryEntry::File {
        path: &parent,
        bytes: b"xx",
    }];
    assert!(matches!(
        guard.stage_directory(leaf("large.next"), &bytes, 1),
        Err(MutationError::Invalid)
    ));
    assert!(matches!(
        guard.stage_directory(leaf("large.next"), &[], MAX_BYTES + 1),
        Err(MutationError::Invalid)
    ));
    assert!(matches!(
        guard.stage_directory(leaf(".mayhem-ntfs.lock"), &[], 0),
        Err(MutationError::Invalid)
    ));
    assert!(!f.0.join("many.next").exists());
    assert!(!f.0.join("large.next").exists());
}

#[test]
fn ntfs_directory_first_install_flushes_nested_files_and_reopens_exact_tree() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    let wizard = path(&["wizard.json"]);
    let state = path(&["state"]);
    let runtime = path(&["runtime"]);
    let worker = path(&["worker"]);
    let nested = path(&["state", "nested"]);
    let draft = path(&["state", "nested", "draft.json"]);
    let entries = [
        DirectoryEntry::File {
            path: &draft,
            bytes: b"exact original draft",
        },
        DirectoryEntry::Directory { path: &runtime },
        DirectoryEntry::File {
            path: &wizard,
            bytes: b"retained wizard",
        },
        DirectoryEntry::Directory { path: &worker },
        DirectoryEntry::Directory { path: &nested },
        DirectoryEntry::Directory { path: &state },
    ];
    let mut pending = guard
        .stage_directory(leaf("install.next"), &entries, 128)
        .unwrap();
    let original = pending.identity();
    assert!(!f.0.join("bundle").exists());
    assert_eq!(pending.publish(leaf("bundle")).unwrap(), original);
    assert!(pending.is_committed());
    assert_eq!(pending.reconcile().unwrap(), original);
    assert_eq!(
        pending.publish(leaf("other")),
        Err(MutationError::CommitUnknown)
    );
    drop(pending);
    assert!(!f.0.join("install.next").exists());
    assert!(!f.0.join("other").exists());
    assert_eq!(
        &*read_private_file(&f.0.join(r"bundle\state\nested\draft.json"), 128).unwrap(),
        b"exact original draft"
    );
    assert_eq!(
        &*read_private_file(&f.0.join(r"bundle\wizard.json"), 128).unwrap(),
        b"retained wizard"
    );
    assert!(fs::read_dir(f.0.join(r"bundle\runtime"))
        .unwrap()
        .next()
        .is_none());
    assert!(fs::read_dir(f.0.join(r"bundle\worker"))
        .unwrap()
        .next()
        .is_none());
    validate_private_directory(&f.0.join("bundle")).unwrap();
}

#[test]
fn ntfs_directory_existing_destination_never_merges_or_replaces_even_when_empty() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    for destination in ["empty", "occupied"] {
        let p = path(&["original.json"]);
        let entries = if destination == "empty" {
            vec![]
        } else {
            vec![DirectoryEntry::File {
                path: &p,
                bytes: b"original",
            }]
        };
        let mut original = guard
            .stage_directory(leaf("original.next"), &entries, 16)
            .unwrap();
        original.publish(leaf(destination)).unwrap();
        let original_id = original.identity();
        drop(original);
        let mut candidate = guard
            .stage_directory(leaf("candidate.next"), &[], 0)
            .unwrap();
        assert_eq!(
            candidate.publish(leaf(destination)),
            Err(MutationError::Conflict)
        );
        assert!(candidate.attempted_destination().is_none());
        let held =
            native::inspect_directory(dir.pinned.file(), &dir.pinned.owner, &leaf(destination))
                .unwrap()
                .unwrap();
        assert_eq!(identity(&held, &dir.pinned.owner).unwrap(), original_id);
        drop(held);
        drop(candidate);
        // Test cleanup only. Production API deliberately has no deletion operation.
        fs::remove_dir(f.0.join("candidate.next")).unwrap();
    }
    assert_eq!(
        &*read_private_file(&f.0.join(r"occupied\original.json"), 16).unwrap(),
        b"original"
    );
    assert!(fs::read_dir(f.0.join("empty")).unwrap().next().is_none());
    // A regular file target also cannot be treated as a replaceable directory.
    let mut file = guard.prepare(leaf("file.next"), b"retained", 16).unwrap();
    file.publish(leaf("file-target"), PublishMode::CreateNew)
        .unwrap();
    drop(file);
    let mut candidate = guard
        .stage_directory(leaf("candidate.next"), &[], 0)
        .unwrap();
    assert_eq!(
        candidate.publish(leaf("file-target")),
        Err(MutationError::Protection)
    );
    assert!(candidate.attempted_destination().is_none());
}

#[test]
fn ntfs_directory_fault_boundaries_keep_complete_tree_and_exact_original_recovery() {
    for failed in [Stage::BeforeRename, Stage::AfterRename, Stage::AfterFlush] {
        let f = Fixture::new();
        let dir = NtfsDirectory::open_existing(&f.0).unwrap();
        let mut guard = dir.try_lock().unwrap();
        let p = path(&["wizard.json"]);
        let entries = [DirectoryEntry::File {
            path: &p,
            bytes: b"complete",
        }];
        let mut pending = guard
            .stage_directory(leaf("install.next"), &entries, 8)
            .unwrap();
        let original = pending.identity();
        let result = pending.publish_inner(leaf("bundle"), |stage| {
            if stage == failed {
                Err(MutationError::Storage)
            } else {
                Ok(())
            }
        });
        assert!(!pending.is_committed());
        assert_eq!(pending.identity(), original);
        if failed == Stage::BeforeRename {
            assert_eq!(result, Err(MutationError::Storage));
            assert!(pending.attempted_destination().is_none());
            assert!(!f.0.join("bundle").exists());
            drop(pending);
            assert_eq!(
                &*read_private_file(&f.0.join(r"install.next\wizard.json"), 8).unwrap(),
                b"complete"
            );
        } else {
            assert_eq!(result, Err(MutationError::CommitUnknown));
            assert_eq!(pending.attempted_destination(), Some(&leaf("bundle")));
            assert_eq!(
                pending.publish(leaf("other")),
                Err(MutationError::CommitUnknown)
            );
            assert_eq!(pending.reconcile().unwrap(), original);
            drop(pending);
            assert_eq!(
                &*read_private_file(&f.0.join(r"bundle\wizard.json"), 8).unwrap(),
                b"complete"
            );
            assert!(!f.0.join("other").exists());
        }
    }
}

#[test]
fn ntfs_directory_rename_collision_after_precheck_remains_uncertain_and_never_retries() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    let mut pending = guard.stage_directory(leaf("install.next"), &[], 0).unwrap();
    let result = pending.publish_inner(leaf("bundle"), |stage| {
        if stage == Stage::BeforeRename {
            // Simulate a competing same-user writer outside the cooperative lock.
            let rival =
                native::create_directory(dir.pinned.file(), &dir.pinned.owner, &leaf("bundle"))?;
            native::flush(&rival)?;
        }
        Ok(())
    });
    assert_eq!(result, Err(MutationError::CommitUnknown));
    assert_eq!(pending.reconcile(), Err(MutationError::CommitUnknown));
    assert_eq!(
        pending.publish(leaf("other")),
        Err(MutationError::CommitUnknown)
    );
    drop(pending);
    assert!(f.0.join("install.next").is_dir());
    assert!(f.0.join("bundle").is_dir());
    assert!(!f.0.join("other").exists());
}
