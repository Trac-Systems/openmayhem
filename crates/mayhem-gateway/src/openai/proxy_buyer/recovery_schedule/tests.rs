use super::*;

fn page(ids: &[String], schedule: &Schedule) -> Vec<String> {
    ids.iter()
        .filter(|id| schedule.after().is_none_or(|after| id.as_str() > after))
        .take(PAGE)
        .cloned()
        .collect()
}

#[test]
fn retries_back_off_from_completion_and_never_give_up() {
    let mut schedule = Schedule::default();
    let mut now = Instant::now();
    for attempt in 0..100 {
        assert!(schedule.start("original-purchase", now));
        assert!(!schedule.start("another-purchase", now));
        now += Duration::from_secs(90); // Slow reads do not consume the delay.
        assert!(!schedule.available(now));
        schedule.finish("original-purchase", true, now);
        let delay = TICK.saturating_mul(1u32 << attempt.min(5)).min(MAX_DELAY);
        assert_eq!(schedule.retries[0].delay, delay);
        assert!(!schedule.start("original-purchase", now + delay - Duration::from_nanos(1)));
        now += delay;
    }
    assert!(schedule.start("original-purchase", now));
    schedule.finish("original-purchase", false, now);
    assert!(
        schedule.retries.is_empty(),
        "terminal jobs need no retry hint"
    );
}

#[test]
fn every_job_including_short_tail_rotates_before_revisiting_a_due_head() {
    for count in [6, PAGE, PAGE + 7, PAGE * 3 + 1] {
        let ids: Vec<_> = (0..count).map(|i| format!("job-{i:04}")).collect();
        let mut schedule = Schedule::default();
        let mut now = Instant::now();
        let mut selected = Vec::new();
        for _ in 0..(count + 1) * 3 {
            if let Some(id) = schedule.candidate(&page(&ids, &schedule), now) {
                schedule.visited(&id);
                assert!(schedule.start(&id, now));
                schedule.finish(&id, true, now);
                selected.push(id);
            }
            // Make all previously seen IDs due, to catch tail starvation.
            now += MAX_DELAY;
        }
        assert!(selected.len() >= count * 2);
        assert_eq!(&selected[..count], ids);
        assert_eq!(&selected[count..count * 2], ids);
    }
}

#[test]
fn capacity_refusal_does_not_advance_past_the_next_candidate() {
    let mut schedule = Schedule::default();
    let now = Instant::now();
    let ids = vec!["a".into(), "b".into(), "c".into()];
    schedule.visited("a");
    for _ in 0..100 {
        assert_eq!(
            schedule.candidate(&page(&ids, &schedule), now).as_deref(),
            Some("b")
        );
        // Caller could not claim capacity: no visited/start/finish.
        assert_eq!(schedule.after(), Some("a"));
    }
    schedule.visited("b"); // A successful claim, or an existing foreground owner.
    assert_eq!(
        schedule.candidate(&page(&ids, &schedule), now).as_deref(),
        Some("c")
    );
}

#[test]
fn cooling_pages_advance_and_wrap_without_blocking_new_work() {
    let mut schedule = Schedule::default();
    let now = Instant::now();
    let ids: Vec<_> = (0..PAGE + 1).map(|i| format!("job-{i:04}")).collect();
    for id in &ids[..PAGE] {
        schedule.retries.push_back(Retry {
            id: id.clone(),
            delay: MAX_DELAY,
            due: now + MAX_DELAY,
        });
    }
    assert!(schedule.candidate(&page(&ids, &schedule), now).is_none());
    assert_eq!(schedule.after(), Some(ids[PAGE - 1].as_str()));
    let tail = schedule.candidate(&page(&ids, &schedule), now).unwrap();
    assert_eq!(tail, ids[PAGE]);
    schedule.visited(&tail);
    assert!(schedule.candidate(&page(&ids, &schedule), now).is_none());
    assert_eq!(schedule.after(), None);
    assert_eq!(
        schedule.candidate(&page(&ids, &schedule), now + MAX_DELAY),
        Some(ids[0].clone())
    );
}

#[test]
fn retry_cache_is_bounded_and_eviction_does_not_exclude_an_obligation() {
    let mut schedule = Schedule::default();
    let mut now = Instant::now();
    for i in 0..RETRIES * 10 {
        let id = format!("job-{i:04}");
        assert!(schedule.start(&id, now));
        schedule.finish(&id, true, now);
        assert!(schedule.retries.len() <= RETRIES);
        now += TICK;
    }
    assert_eq!(schedule.retries.len(), RETRIES);
    assert!(schedule.retries.iter().all(|retry| retry.id != "job-0000"));
    assert!(schedule.start("job-0000", now));
}

#[test]
fn background_admission_preserves_a_foreground_slot_without_reducing_foreground_capacity() {
    for sessions in [2, 8, 64] {
        let slots = Arc::new(Semaphore::new(sessions));
        let background = claim_slot(&slots, sessions).unwrap();
        assert_eq!(slots.available_permits(), sessions - 1);
        let foreground = slots
            .clone()
            .try_acquire_many_owned((sessions - 1) as u32)
            .unwrap();
        assert_eq!(slots.available_permits(), 0);
        drop(background);
        assert_eq!(slots.available_permits(), 1);
        assert!(
            claim_slot(&slots, sessions).is_err(),
            "last slot belongs to foreground"
        );
        assert_eq!(
            slots.available_permits(),
            1,
            "failed probe must release all permits"
        );
        let last_foreground = slots.clone().try_acquire_owned().unwrap();
        drop((foreground, last_foreground));
        assert_eq!(slots.available_permits(), sessions);
    }
}

#[test]
fn single_session_allows_recovery_and_a_foreground_window_even_with_a_backlog() {
    let slots = Arc::new(Semaphore::new(1));
    let mut schedule = Schedule::default();
    let now = Instant::now();
    let background = claim_slot(&slots, 1).unwrap();
    assert!(schedule.start("a", now));
    assert!(slots.clone().try_acquire_owned().is_err());
    schedule.finish("a", true, now);
    drop(background);
    assert!(!schedule.start("b", now + TICK - Duration::from_nanos(1)));
    let foreground = slots.clone().try_acquire_owned().unwrap();
    assert!(claim_slot(&slots, 1).is_err());
    drop(foreground);
    assert!(schedule.start("b", now + TICK));
    assert!(claim_slot(&slots, 1).is_ok());
}

#[test]
fn dropping_an_owned_claim_releases_capacity_and_backs_off_the_original_job() {
    use super::super::Claim;
    use std::{collections::BTreeSet, sync::Mutex};

    let slots = Arc::new(Semaphore::new(8));
    let active = Arc::new(Mutex::new(BTreeSet::from(["original".to_owned()])));
    let recovery = Arc::new(Mutex::new(Schedule::default()));
    assert!(recovery.lock().unwrap().start("original", Instant::now()));
    let claim = Claim {
        id: "original".into(),
        active: active.clone(),
        recovery: Some(recovery.clone()),
        pending: true,
        _permit: claim_slot(&slots, 8).unwrap(),
    };
    assert_eq!(slots.available_permits(), 7);
    drop(claim); // Includes an early storage failure or owned task cancellation.
    assert!(active.lock().unwrap().is_empty());
    assert_eq!(slots.available_permits(), 8);
    let mut recovery = recovery.lock().unwrap();
    assert!(!recovery.available(Instant::now()));
    assert!(!recovery.start("original", Instant::now()));
    assert!(recovery.start("original", Instant::now() + TICK));
}
