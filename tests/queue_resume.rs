//! Queue behaviour: caps, dedup, claim, cancel, resume-after-kill.

use kiwimanga::db;
use kiwimanga::queue::{self, EnqueueOutcome, JobKind, JobPayload};
use kiwimanga::sources::ChapterInfo;

async fn memdb() -> sqlx::sqlite::SqlitePool {
    db::connect_with("sqlite::memory:", 1).await.unwrap()
}

fn chapter(i: usize) -> ChapterInfo {
    ChapterInfo {
        id: format!("c{i}"),
        volume: Some("1".to_string()),
        number: Some(i.to_string()),
        title: None,
        lang: "en".to_string(),
        pages: 10,
    }
}

fn payload(title_id: &str, n: usize) -> JobPayload {
    JobPayload {
        source: "mangadex".to_string(),
        title_id: title_id.to_string(),
        title_name: format!("Title {title_id}"),
        kind: JobKind::Range { from: "1".to_string(), to: n.to_string() },
        chapters: (1..=n).map(chapter).collect(),
        pack_mode: "merged".to_string(),
        chapter_lang: "en".to_string(),
        want_lang: "en".to_string(),
        lang_fallback: false,
    }
}

#[tokio::test]
async fn accept_and_global_positions() {
    let db = memdb().await;
    let p1 = payload("a", 3);
    let id1 = match queue::enqueue(&db, 100, &p1).await.unwrap() {
        EnqueueOutcome::Accepted { id, position } => {
            assert_eq!(position, 1);
            id
        }
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    };
    let p2 = payload("b", 2);
    match queue::enqueue(&db, 100, &p2).await.unwrap() {
        EnqueueOutcome::Accepted { id, position } => {
            assert_eq!(position, 2);
            assert!(id > id1);
        }
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    }
}

#[tokio::test]
async fn duplicate_is_deduped() {
    let db = memdb().await;
    let p = payload("a", 3);
    let first = match queue::enqueue(&db, 7, &p).await.unwrap() {
        EnqueueOutcome::Accepted { id, .. } => id,
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    };
    match queue::enqueue(&db, 7, &p).await.unwrap() {
        EnqueueOutcome::Duplicate { id } => assert_eq!(id, first),
        other => panic!("expected duplicate, got {}", outcome_name(&other)),
    }
    // Same payload from another chat is a different job.
    match queue::enqueue(&db, 8, &p).await.unwrap() {
        EnqueueOutcome::Accepted { .. } => {}
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    }
}

#[tokio::test]
async fn per_user_caps() {
    let db = memdb().await;
    for t in ["a", "b", "c"] {
        let p = payload(t, 1);
        assert!(matches!(
            queue::enqueue(&db, 42, &p).await.unwrap(),
            EnqueueOutcome::Accepted { .. }
        ));
    }
    let p = payload("d", 1);
    match queue::enqueue(&db, 42, &p).await.unwrap() {
        EnqueueOutcome::RejectActive { active, max } => {
            assert_eq!((active, max), (3, 3));
        }
        other => panic!("expected reject-active, got {}", outcome_name(&other)),
    }
    // Chapter cap is per job, checked before the active cap matters.
    let big = payload("big", 201);
    match queue::enqueue(&db, 43, &big).await.unwrap() {
        EnqueueOutcome::RejectTooMany { n, max } => {
            assert_eq!((n, max), (201, 200));
        }
        other => panic!("expected reject-too-many, got {}", outcome_name(&other)),
    }
    // Exactly 200 is fine.
    let edge = payload("edge", 200);
    assert!(matches!(
        queue::enqueue(&db, 43, &edge).await.unwrap(),
        EnqueueOutcome::Accepted { .. }
    ));
}

#[tokio::test]
async fn claim_resume_after_kill() {
    let db = memdb().await;
    let p = payload("a", 2);
    let id = match queue::enqueue(&db, 1, &p).await.unwrap() {
        EnqueueOutcome::Accepted { id, .. } => id,
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    };
    // Worker claims it...
    let claimed = queue::claim_next(&db).await.unwrap().expect("job claimed");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, "running");
    // ...then the process dies (kill -9): nothing else can claim it...
    assert!(queue::claim_next(&db).await.unwrap().is_none());
    // ...but after restart the recovery resets running -> pending...
    let n = queue::reset_running_to_pending(&db).await.unwrap();
    assert_eq!(n, 1);
    // ...and the job is claimable again.
    let again = queue::claim_next(&db).await.unwrap().expect("re-claimed");
    assert_eq!(again.id, id);
}

#[tokio::test]
async fn cancel_flow() {
    let db = memdb().await;
    let p = payload("a", 1);
    let id = match queue::enqueue(&db, 5, &p).await.unwrap() {
        EnqueueOutcome::Accepted { id, .. } => id,
        other => panic!("expected accepted, got {}", outcome_name(&other)),
    };
    assert!(queue::newest_active(&db, 5).await.unwrap().is_some());
    let prev = queue::request_cancel(&db, id).await.unwrap();
    assert_eq!(prev.as_deref(), Some("pending"));
    assert!(queue::newest_active(&db, 5).await.unwrap().is_none());
    // Second cancel is a noop.
    assert!(queue::request_cancel(&db, id).await.unwrap().is_none());
}

fn outcome_name(o: &EnqueueOutcome) -> &'static str {
    match o {
        EnqueueOutcome::Accepted { .. } => "accepted",
        EnqueueOutcome::Duplicate { .. } => "duplicate",
        EnqueueOutcome::RejectActive { .. } => "reject-active",
        EnqueueOutcome::RejectTooMany { .. } => "reject-too-many",
    }
}
