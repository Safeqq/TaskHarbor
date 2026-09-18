use std::env;
use std::time::Duration;

use sqlx::PgPool;
use taskharbor_adapters::PgJobRepository;
use taskharbor_core::{JobName, JobStatus};
use taskharbor_worker::run;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn finishes_the_active_job_before_graceful_shutdown() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 3)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("migrations should succeed");
    let setup_pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query("TRUNCATE job_attempts, jobs RESTART IDENTITY CASCADE")
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let job = repository
        .create(JobName::new("graceful shutdown").expect("test name should be valid"))
        .await
        .expect("test job should be created");
    let job_id = job.job().id();
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let worker = tokio::spawn(run(repository.clone(), shutdown_receiver));

    timeout(Duration::from_secs(5), async {
        loop {
            let current = repository
                .get(job_id)
                .await
                .expect("job state should be readable")
                .expect("test job should exist");
            if current.job().status() == JobStatus::Running {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker should claim the job");

    shutdown_sender
        .send(true)
        .expect("worker should still receive shutdown");
    timeout(Duration::from_secs(5), worker)
        .await
        .expect("worker should stop after the active job")
        .expect("worker task should not panic")
        .expect("worker should stop without a repository error");

    let completed = repository
        .get(job_id)
        .await
        .expect("completed state should be readable")
        .expect("completed job should exist");
    assert_eq!(completed.job().status(), JobStatus::Succeeded);
    assert_eq!(completed.progress_completed(), completed.progress_total());

    let attempt_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM job_attempts WHERE job_id = $1 AND state = 'succeeded'",
    )
    .bind(i64::try_from(job_id.get()).expect("test ID should fit BIGINT"))
    .fetch_one(&setup_pool)
    .await
    .expect("attempt count should be readable");
    assert_eq!(attempt_count, 1);
}
