use super::*;
use crate::private_files::mutation::{tests::Fixture, DirectoryEntry};
use std::fs;
fn leaf(v: &str) -> LeafName {
    LeafName::new(v).unwrap()
}
fn commit(g: &mut NtfsGuard<'_>, temp: &str, name: &str, bytes: &[u8]) {
    g.prepare(leaf(temp), bytes, 64)
        .unwrap()
        .publish(leaf(name), PublishMode::CreateNew)
        .unwrap();
}

#[test]
fn ntfs_owned_guard_retains_lock_and_cleans_only_distinct_unpublished_fixed_slot() {
    let f = Fixture::new();
    let mut guard = NtfsDirectory::open_existing(&f.0)
        .unwrap()
        .into_lock()
        .unwrap();
    assert!(matches!(
        NtfsDirectory::open_existing(&f.0).unwrap().into_lock(),
        Err(MutationError::Busy)
    ));
    commit(&mut guard, "first.next", "draft.json", b"original");
    drop(
        guard
            .prepare(leaf("draft.next"), b"incomplete candidate", 64)
            .unwrap(),
    );
    guard
        .discard_uncommitted(&leaf("draft.json"), &leaf("draft.next"))
        .unwrap();
    assert_eq!(
        &*guard.read(&leaf("draft.json"), 64).unwrap().unwrap(),
        b"original"
    );
    assert!(!f.0.join("draft.next").exists());
    // Missing original is distinct from a malformed/reparse original.
    drop(
        guard
            .prepare(leaf("absent.next"), b"candidate", 64)
            .unwrap(),
    );
    guard
        .discard_uncommitted(&leaf("absent.json"), &leaf("absent.next"))
        .unwrap();
    assert!(!f.0.join("absent.json").exists());
    assert!(!f.0.join("absent.next").exists());
}

#[test]
fn ntfs_cleanup_refuses_aliases_nonregular_original_and_current_uncertain_mutation() {
    let f = Fixture::new();
    let mut guard = NtfsDirectory::open_existing(&f.0)
        .unwrap()
        .into_lock()
        .unwrap();
    commit(&mut guard, "first.next", "draft.json", b"original");
    assert_eq!(
        guard.discard_uncommitted(&leaf("draft.json"), &leaf("draft.json")),
        Err(MutationError::Invalid)
    );
    fs::hard_link(f.0.join("draft.json"), f.0.join("draft.next")).unwrap();
    assert_eq!(
        guard.discard_uncommitted(&leaf("draft.json"), &leaf("draft.next")),
        Err(MutationError::Protection)
    );
    fs::remove_file(f.0.join("draft.next")).unwrap();
    drop(guard.prepare(leaf("draft.next"), b"candidate", 64).unwrap());
    let mut directory = guard
        .stage_directory(leaf("directory.next"), &[], 0)
        .unwrap();
    directory.publish(leaf("not-a-file")).unwrap();
    drop(directory);
    assert_eq!(
        guard.discard_uncommitted(&leaf("not-a-file"), &leaf("draft.next")),
        Err(MutationError::Protection)
    );
    guard
        .discard_uncommitted(&leaf("draft.json"), &leaf("draft.next"))
        .unwrap();
    let held = Pinned::open(&f.0.join("draft.json"), false).unwrap();
    let mut pending = guard.prepare(leaf("draft.next"), b"new", 64).unwrap();
    assert_eq!(
        pending.publish(leaf("draft.json"), PublishMode::Replace),
        Err(MutationError::CommitUnknown)
    );
    drop(pending);
    assert_eq!(
        guard.discard_uncommitted(&leaf("draft.json"), &leaf("draft.next")),
        Err(MutationError::CommitUnknown)
    );
    assert!(matches!(
        guard.prepare(leaf("other.next"), b"new", 64),
        Err(MutationError::CommitUnknown)
    ));
    drop(held);
    drop(guard);
    let mut recovered = NtfsDirectory::open_existing(&f.0)
        .unwrap()
        .into_lock()
        .unwrap();
    recovered
        .discard_uncommitted(&leaf("draft.json"), &leaf("draft.next"))
        .unwrap();
    assert_eq!(
        &*recovered.read(&leaf("draft.json"), 64).unwrap().unwrap(),
        b"original"
    );
}

#[test]
fn ntfs_staged_inspection_uses_existing_private_loader_then_exact_handle_publication() {
    let f = Fixture::new();
    let mut guard = NtfsDirectory::open_existing(&f.0)
        .unwrap()
        .into_lock()
        .unwrap();
    let path = [leaf("wizard.json")];
    let entries = [DirectoryEntry::File {
        path: &path,
        bytes: b"exact inspected bytes",
    }];
    let mut stage = guard
        .stage_directory(leaf("bundle.next"), &entries, 64)
        .unwrap();
    let original = stage.identity();
    stage.prepare_for_inspection().unwrap();
    assert_eq!(
        &*read_private_file(&f.0.join(r"bundle.next\wizard.json"), 64).unwrap(),
        b"exact inspected bytes"
    );
    assert_eq!(stage.publish(leaf("bundle")).unwrap(), original);
    assert_eq!(
        stage.prepare_for_inspection(),
        Err(MutationError::CommitUnknown)
    );
    drop(stage);
    assert_eq!(
        &*read_private_file(&f.0.join(r"bundle\wizard.json"), 64).unwrap(),
        b"exact inspected bytes"
    );
}

#[test]
fn ntfs_inspected_tree_revalidates_original_directories_after_local_validation() {
    let f = Fixture::new();
    let mut guard = NtfsDirectory::open_existing(&f.0)
        .unwrap()
        .into_lock()
        .unwrap();
    let worker = [leaf("worker")];
    let entries = [DirectoryEntry::Directory { path: &worker }];
    let mut stage = guard
        .stage_directory(leaf("bundle.next"), &entries, 0)
        .unwrap();
    stage.prepare_for_inspection().unwrap();
    let root = f.0.join("bundle.next");
    // An inspector cannot replace the retained staged root. Local tokenizer
    // validation can create/remove a temporary file inside its declared worker.
    assert!(fs::rename(&root, f.0.join("moved")).is_err());
    fs::write(root.join(r"worker\temporary"), b"synthetic image").unwrap();
    fs::remove_file(root.join(r"worker\temporary")).unwrap();
    fs::rename(root.join("worker"), root.join("parked")).unwrap();
    fs::create_dir(root.join("worker")).unwrap();
    assert_eq!(
        stage.publish(leaf("bundle")),
        Err(MutationError::Protection)
    );
    assert!(stage.attempted_destination().is_none());
    assert!(!f.0.join("bundle").exists());
    fs::remove_dir(root.join("worker")).unwrap();
    fs::rename(root.join("parked"), root.join("worker")).unwrap();
    stage.publish(leaf("bundle")).unwrap();
    assert!(f.0.join(r"bundle\worker").is_dir());
}
