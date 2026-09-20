use std::env;
use std::time::Duration as StdDuration;

use taskharbor_adapters::{
    AttemptStatus, PgJobRepository, ReclaimDisposition, RepositoryError, WorkerId,
    WorkerRegistration, WorkerStatus,
};
use taskharbor_core::{JobName, JobStatus};
use time::Duration;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn leases_reclaim_expired_attempts_and_fence_stale_workers() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 10)
        .await
        .expect("test database should be reachable");
    repository.migrate().await.expect("migrations should run");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("test pool should connect");
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("test tables should reset");

    let database_now = sqlx::query_scalar::<_, time::OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&pool)
        .await
        .expect("database clock should be readable");
    let base = database_now + Duration::minutes(1);
    let first_worker = register_worker(&repository, "lease-worker-a", base).await;
    let second_worker = register_worker(&repository, "lease-worker-b", base).await;
    let first_job = repository
        .create(JobName::new("first concurrent claim").expect("test name should be valid"))
        .await
        .expect("first job should be created");
    let second_job = repository
        .create(JobName::new("second concurrent claim").expect("test name should be valid"))
        .await
        .expect("second job should be created");

    let (first_claim, second_claim) = tokio::join!(
        repository.claim_next_at(first_worker, base),
        repository.claim_next_at(second_worker, base),
    );
    let first_claim = first_claim
        .expect("first concurrent claim should succeed")
        .expect("first worker should receive a job");
    let second_claim = second_claim
        .expect("second concurrent claim should succeed")
        .expect("second worker should receive a job");
    assert_ne!(first_claim.job_id(), second_claim.job_id());
    assert_eq!(
        [first_claim.job_id(), second_claim.job_id()]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        [first_job.job().id(), second_job.job().id()]
            .into_iter()
            .collect()
    );

    let renewed_until = repository
        .renew_lease_at(&first_claim, base + Duration::seconds(4))
        .await
        .expect("active owner should renew its lease");
    assert_eq!(renewed_until, base + Duration::seconds(9));

    let reclaimed = repository
        .reclaim_next_expired_attempt_at(base + Duration::seconds(5))
        .await
        .expect("expired attempt should be reclaimed")
        .expect("one unrenewed attempt should be expired");
    assert_eq!(reclaimed.job_id(), second_claim.job_id());
    assert_eq!(reclaimed.worker_id(), second_worker);
    assert_eq!(
        reclaimed.disposition(),
        ReclaimDisposition::RetryScheduled {
            available_at: base + Duration::seconds(6)
        }
    );
    assert!(matches!(
        repository.update_progress(&second_claim, 1).await,
        Err(RepositoryError::ClaimLost)
    ));
    assert!(matches!(
        repository
            .complete(&second_claim, StdDuration::from_millis(10))
            .await,
        Err(RepositoryError::ClaimLost)
    ));

    let recovery_worker = register_worker(
        &repository,
        "lease-worker-recovery",
        base + Duration::seconds(6),
    )
    .await;
    let recovery_claim = repository
        .claim_next_at(recovery_worker, base + Duration::seconds(6))
        .await
        .expect("recovery claim should succeed")
        .expect("requeued job should be eligible after backoff");
    assert_eq!(recovery_claim.job_id(), second_claim.job_id());
    assert_ne!(recovery_claim.claim_token(), second_claim.claim_token());
    repository
        .complete(&recovery_claim, StdDuration::from_millis(8))
        .await
        .expect("new owner should complete the recovered job");
    repository
        .complete(&first_claim, StdDuration::from_millis(9))
        .await
        .expect("renewed owner should still complete its job");

    let recovered = repository
        .get(second_claim.job_id())
        .await
        .expect("recovered job should be readable")
        .expect("recovered job should exist");
    assert_eq!(recovered.job().status(), JobStatus::Succeeded);
    let attempts = recovered.attempts().collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status(), AttemptStatus::Failed);
    assert_eq!(attempts[0].error_kind(), Some("transient"));
    assert_eq!(attempts[0].worker_name(), Some("lease-worker-b"));
    assert_eq!(attempts[1].status(), AttemptStatus::Succeeded);
    assert_eq!(attempts[1].worker_name(), Some("lease-worker-recovery"));

    let cancelling = repository
        .create(JobName::new("expired cancellation").expect("test name should be valid"))
        .await
        .expect("cancel test job should be created");
    let cancel_claim = repository
        .claim_next_at(first_worker, base + Duration::seconds(10))
        .await
        .expect("cancel test claim should succeed")
        .expect("cancel test job should be claimable");
    repository
        .request_cancel(cancelling.job().id())
        .await
        .expect("cancel should be requested")
        .expect("cancel test job should exist");
    assert!(matches!(
        repository
            .renew_lease_at(&cancel_claim, base + Duration::seconds(16))
            .await,
        Err(RepositoryError::ClaimLost)
    ));
    let cancelled = repository
        .reclaim_next_expired_attempt_at(base + Duration::seconds(16))
        .await
        .expect("expired cancellation should be reclaimed")
        .expect("cancelled attempt should be found");
    assert_eq!(cancelled.disposition(), ReclaimDisposition::Cancelled);
    assert_eq!(
        repository
            .get(cancelling.job().id())
            .await
            .expect("cancelled job should be readable")
            .expect("cancelled job should exist")
            .job()
            .status(),
        JobStatus::Cancelled
    );

    let exhausted = repository
        .create(JobName::new("expired final attempt").expect("test name should be valid"))
        .await
        .expect("limit test job should be created");
    sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
        .bind(i64::try_from(exhausted.job().id().get()).expect("job ID should fit BIGINT"))
        .execute(&pool)
        .await
        .expect("attempt limit should be configured");
    let exhausted_claim = repository
        .claim_next_at(first_worker, base + Duration::seconds(20))
        .await
        .expect("limit test claim should succeed")
        .expect("limit test job should be claimable");
    let exhausted_reclaim = repository
        .reclaim_next_expired_attempt_at(base + Duration::seconds(26))
        .await
        .expect("last attempt should be reclaimed")
        .expect("last attempt should expire");
    assert_eq!(exhausted_reclaim.job_id(), exhausted_claim.job_id());
    assert_eq!(exhausted_reclaim.disposition(), ReclaimDisposition::Failed);
    assert_eq!(
        repository
            .get(exhausted.job().id())
            .await
            .expect("failed job should be readable")
            .expect("failed job should exist")
            .job()
            .status(),
        JobStatus::Failed
    );

    let offline_worker = register_worker(
        &repository,
        "offline-worker",
        database_now - Duration::seconds(120),
    )
    .await;
    let offline = repository
        .get_worker(offline_worker)
        .await
        .expect("offline worker should be readable")
        .expect("offline worker should exist");
    assert_eq!(offline.status(), WorkerStatus::Offline);
}

async fn register_worker(
    repository: &PgJobRepository,
    name: &str,
    as_of: time::OffsetDateTime,
) -> WorkerId {
    let worker_id = WorkerId::new();
    repository
        .register_worker_at(
            &WorkerRegistration {
                id: worker_id,
                name: name.into(),
                concurrency_limit: 2,
                lease_duration: StdDuration::from_secs(5),
                heartbeat_ttl: StdDuration::from_secs(60),
            },
            as_of,
        )
        .await
        .expect("test worker should register");
    worker_id
}
