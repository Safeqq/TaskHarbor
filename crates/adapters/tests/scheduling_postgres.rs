use std::env;
use std::time::Duration as StdDuration;

use taskharbor_adapters::{
    JobSettings, NewImageJob, NewInputArtifact, NewSchedule, OWNER_USER_ID, PgJobRepository,
    ScheduleOccurrenceOutcome, UpdateSchedule, WorkerId, WorkerRegistration,
};
use taskharbor_core::{JobName, JobPriority, JobStatus};
use time::{Duration, OffsetDateTime};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn scheduling_is_anchored_prioritized_idempotent_and_overlap_safe() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 8)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("test migrations should run");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("test pool should connect");
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("test tables should reset");

    let base =
        OffsetDateTime::from_unix_timestamp(1_900_000_000).expect("test timestamp should be valid");
    let worker_id = WorkerId::new();
    repository
        .register_worker_at(
            &WorkerRegistration {
                id: worker_id,
                name: "schedule-test-worker".into(),
                concurrency_limit: 1,
                lease_duration: StdDuration::from_secs(3_600),
                heartbeat_ttl: StdDuration::from_secs(3_600),
            },
            base,
        )
        .await
        .expect("test worker should register");
    let low = create_job(&repository, "low due", base, JobPriority::Low).await;
    let high = create_job(&repository, "high due", base, JobPriority::High).await;
    let future = create_job(
        &repository,
        "normal future",
        base + Duration::hours(1),
        JobPriority::Normal,
    )
    .await;

    let claimed_high = repository
        .claim_next_at(worker_id, base)
        .await
        .expect("priority claim should succeed")
        .expect("high priority job should be due");
    assert_eq!(claimed_high.job_id(), high.job().id());
    cancel_claimed(&repository, &claimed_high).await;

    let claimed_low = repository
        .claim_next_at(worker_id, base)
        .await
        .expect("second claim should succeed")
        .expect("low priority job should remain claimable");
    assert_eq!(claimed_low.job_id(), low.job().id());
    cancel_claimed(&repository, &claimed_low).await;
    assert!(
        repository
            .claim_next_at(worker_id, base + Duration::minutes(59))
            .await
            .expect("early claim check should succeed")
            .is_none()
    );
    repository
        .heartbeat_worker_at(
            worker_id,
            StdDuration::from_secs(3_600),
            base + Duration::hours(1),
        )
        .await
        .expect("test worker heartbeat should renew");
    let claimed_future = repository
        .claim_next_at(worker_id, base + Duration::hours(1))
        .await
        .expect("boundary claim should succeed")
        .expect("job should become eligible exactly at available_at");
    assert_eq!(claimed_future.job_id(), future.job().id());
    cancel_claimed(&repository, &claimed_future).await;

    let schedule = repository
        .create_schedule(new_schedule("catalog schedule", base))
        .await
        .expect("schedule should be created");
    let tick = repository
        .materialize_next_schedule_at(base + Duration::seconds(190))
        .await
        .expect("scheduler tick should succeed")
        .expect("overdue schedule should produce one occurrence");
    assert_eq!(tick.schedule_id(), schedule.id());
    assert_eq!(tick.scheduled_for(), base + Duration::seconds(180));
    assert_eq!(tick.coalesced_slots(), 3);
    let first_occurrence_job = tick.job_id().expect("first occurrence should create a job");
    assert!(
        repository
            .materialize_next_schedule_at(base + Duration::seconds(190))
            .await
            .expect("restarted scheduler check should succeed")
            .is_none(),
        "the same cursor must not create a duplicate occurrence"
    );

    let overlap = repository
        .materialize_next_schedule_at(base + Duration::seconds(250))
        .await
        .expect("overlap tick should succeed")
        .expect("next slot should be recorded");
    assert_eq!(overlap.outcome(), ScheduleOccurrenceOutcome::SkippedOverlap);
    assert!(overlap.job_id().is_none());
    repository
        .request_cancel(first_occurrence_job)
        .await
        .expect("queued occurrence should be cancellable")
        .expect("occurrence job should exist");

    let resumed = repository
        .materialize_next_schedule_at(base + Duration::seconds(310))
        .await
        .expect("resumed tick should succeed")
        .expect("terminal previous occurrence should allow a new job");
    assert_eq!(resumed.outcome(), ScheduleOccurrenceOutcome::Created);
    repository
        .request_cancel(resumed.job_id().expect("resumed slot should create a job"))
        .await
        .expect("resumed occurrence should be cancellable")
        .expect("resumed occurrence should exist");

    repository
        .update_schedule_at(
            schedule.id(),
            UpdateSchedule {
                name: JobName::new("catalog schedule paused")
                    .expect("updated name should be valid"),
                enabled: false,
                interval_seconds: 120,
                anchor_at: base,
                priority: JobPriority::High,
                max_width: 3,
                jpeg_quality: 90,
            },
            base + Duration::seconds(320),
        )
        .await
        .expect("schedule update should succeed")
        .expect("schedule should still exist");
    assert!(
        repository
            .materialize_next_schedule_at(base + Duration::seconds(500))
            .await
            .expect("disabled schedule check should succeed")
            .is_none()
    );

    let concurrent = repository
        .create_schedule(new_schedule(
            "concurrent schedule",
            base + Duration::seconds(600),
        ))
        .await
        .expect("concurrent schedule should be created");
    let first_repository = repository.clone();
    let second_repository = repository.clone();
    let concurrent_time = base + Duration::seconds(600);
    let (first_tick, second_tick) = tokio::join!(
        first_repository.materialize_next_schedule_at(concurrent_time),
        second_repository.materialize_next_schedule_at(concurrent_time),
    );
    let ticks = [first_tick, second_tick]
        .into_iter()
        .filter_map(|result| result.expect("concurrent scheduler call should succeed"))
        .collect::<Vec<_>>();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].schedule_id(), concurrent.id());

    let generated_job_id = ticks[0]
        .job_id()
        .expect("concurrent schedule should create one job");
    repository
        .request_cancel(generated_job_id)
        .await
        .expect("generated job should be cancellable")
        .expect("generated job should exist");
    let edited = repository
        .update_schedule_at(
            concurrent.id(),
            UpdateSchedule {
                name: JobName::new("edited future schedule").expect("edited name should be valid"),
                enabled: true,
                interval_seconds: 120,
                anchor_at: base + Duration::seconds(700),
                priority: JobPriority::High,
                max_width: 3,
                jpeg_quality: 90,
            },
            base + Duration::seconds(650),
        )
        .await
        .expect("future edit should succeed")
        .expect("edited schedule should exist");
    assert_eq!(edited.next_run_at(), base + Duration::seconds(700));
    assert!(
        repository
            .materialize_next_schedule_at(base + Duration::seconds(699))
            .await
            .expect("pre-anchor tick should succeed")
            .is_none()
    );
    let edited_tick = repository
        .materialize_next_schedule_at(base + Duration::seconds(700))
        .await
        .expect("edited schedule tick should succeed")
        .expect("edited schedule should create a future occurrence");
    let edited_job = repository
        .get(
            edited_tick
                .job_id()
                .expect("edited occurrence should create a job"),
        )
        .await
        .expect("edited job should be readable")
        .expect("edited job should exist");
    assert_eq!(edited_job.priority(), JobPriority::High);
    assert_eq!(edited_job.schedule_id(), Some(concurrent.id()));
    assert_eq!(
        edited_job.scheduled_for(),
        Some(base + Duration::seconds(700))
    );
    assert!(matches!(
        edited_job.settings(),
        JobSettings::ImageResize {
            max_width: 3,
            jpeg_quality: 90
        }
    ));
    assert_eq!(
        edited_job
            .inputs()
            .next()
            .expect("generated job should retain schedule input")
            .storage_key(),
        "inputs/schedules/shared.png"
    );

    let history = repository
        .get_schedule(schedule.id())
        .await
        .expect("schedule history should load")
        .expect("schedule should exist");
    assert_eq!(history.occurrences().count(), 3);
    assert!(
        history
            .occurrences()
            .any(|occurrence| occurrence.outcome() == ScheduleOccurrenceOutcome::SkippedOverlap)
    );
}

async fn create_job(
    repository: &PgJobRepository,
    name: &str,
    available_at: OffsetDateTime,
    priority: JobPriority,
) -> taskharbor_adapters::JobRecord {
    repository
        .create_image_job(NewImageJob {
            owner_user_id: OWNER_USER_ID,
            name: JobName::new(name).expect("test job name should be valid"),
            max_width: 2,
            jpeg_quality: 85,
            available_at: Some(available_at),
            priority,
            inputs: vec![test_input(&format!("inputs/jobs/{name}.png"))],
            idempotency: None,
        })
        .await
        .expect("test image job should be created")
}

fn new_schedule(name: &str, anchor_at: OffsetDateTime) -> NewSchedule {
    NewSchedule {
        owner_user_id: OWNER_USER_ID,
        name: JobName::new(name).expect("test schedule name should be valid"),
        interval_seconds: 60,
        anchor_at,
        priority: JobPriority::Normal,
        max_width: 2,
        jpeg_quality: 85,
        inputs: vec![test_input("inputs/schedules/shared.png")],
    }
}

fn test_input(storage_key: &str) -> NewInputArtifact {
    NewInputArtifact {
        storage_key: storage_key.into(),
        display_name: "source.png".into(),
        media_type: "image/png".into(),
        byte_size: 8,
        width: 4,
        height: 2,
        checksum_sha256: vec![0; 32],
    }
}

async fn cancel_claimed(repository: &PgJobRepository, claimed: &taskharbor_adapters::ClaimedJob) {
    let requested = repository
        .request_cancel(claimed.job_id())
        .await
        .expect("running job cancellation should succeed")
        .expect("claimed job should exist");
    assert_eq!(requested.job().status(), JobStatus::CancelRequested);
    repository
        .finish_cancelled(claimed, StdDuration::from_millis(1))
        .await
        .expect("claimed job should finish cancellation");
}
