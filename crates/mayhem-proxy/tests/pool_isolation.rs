//! Independent inference/recovery/settlement budgets share only the trusted image.
#![cfg(any(unix, windows))]
use mayhem_proxy::{
    attempts::Digest,
    connector::{config::ErrorProfile, http::WireFormat},
    worker::{
        host::{Pool, PoolLimits},
        DecodeLimits, Error, Init, Session, ABI, RELEASE,
    },
};
use std::time::Duration;
#[cfg(windows)]
#[path = "support/windows_fixture.rs"]
mod windows_fixture;

#[tokio::test]
async fn independent_pools_share_the_pinned_launcher_but_not_capacity() {
    #[cfg(unix)]
    let work = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let work = windows_fixture::PrivateDirectory::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let limits = PoolLimits {
        max_children: 1,
        max_buffer_bytes: 8 * 1024 * 1024,
        startup_timeout: Duration::from_secs(5),
        processing_timeout: Duration::from_secs(3),
    };
    let paid = Pool::new(
        env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
        work.path(),
        limits,
    )
    .unwrap();
    let recovery = paid.with_independent_limits(limits).unwrap();
    let settlement = paid.with_independent_limits(limits).unwrap();
    assert!(paid
        .with_independent_limits(PoolLimits {
            max_children: 0,
            ..limits
        })
        .is_err());
    #[cfg(windows)]
    assert_eq!(
        std::fs::read_dir(work.path()).unwrap().count(),
        2,
        "one pinned image plus its recovery record, not one copy per budget"
    );
    let digest = |n: char| Digest::new(n.to_string().repeat(64)).unwrap();
    let init = Init {
        abi: ABI,
        release: RELEASE.into(),
        session: Session {
            invocation: digest('1'),
            attempt: 1,
            binding_hash: digest('2'),
        },
        format: WireFormat::Json,
        error_profile: ErrorProfile::OpenAi,
        limits: DecodeLimits {
            max_total_bytes: 1024,
            max_event_bytes: 1024,
        },
        semantic_policy: None,
    };
    let first = paid.start(init.clone()).await.unwrap();
    assert!(matches!(
        paid.start(init.clone()).await,
        Err(Error::Capacity)
    ));
    let second = recovery.start(init.clone()).await.unwrap();
    assert!(matches!(
        recovery.start(init.clone()).await,
        Err(Error::Capacity)
    ));
    let third = settlement.start(init.clone()).await.unwrap();
    assert!(matches!(settlement.start(init).await, Err(Error::Capacity)));
    // Removing the creator of the launcher must not invalidate another pool.
    drop(paid);
    first.stop().await.unwrap();
    second.stop().await.unwrap();
    third.stop().await.unwrap();
}
