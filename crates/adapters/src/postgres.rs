use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use taskharbor_core::{Job, JobId, JobName, JobStatus};
use time::OffsetDateTime;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub const DEMO_DELAY_MS: i32 = 500;
const RESULT_MESSAGE: &str = "demo delay completed";

#[derive(Debug, Clone)]
pub struct PgJobRepository {
    pool: PgPool,
}

impl PgJobRepository {
    pub async fn connect(
        database_url: &str,
        max_connections: u32,
    ) -> Result<Self, RepositoryError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;

        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn ping(&self) -> Result<(), RepositoryError> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create(&self, name: JobName) -> Result<JobRecord, RepositoryError> {
        let row = sqlx::query_as::<_, JobRow>(
            r#"
            INSERT INTO jobs (name, delay_ms)
            VALUES ($1, $2)
            RETURNING
                id,
                name,
                state,
                progress_completed,
                progress_total,
                delay_ms,
                result_message,
                result_duration_ms,
                created_at,
                started_at,
                finished_at
            "#,
        )
        .bind(name.as_str())
        .bind(DEMO_DELAY_MS)
        .fetch_one(&self.pool)
        .await?;

        row.try_into()
    }

    pub async fn list(&self) -> Result<Vec<JobRecord>, RepositoryError> {
        let rows = sqlx::query_as::<_, JobRow>(
            r#"
            SELECT
                id,
                name,
                state,
                progress_completed,
                progress_total,
                delay_ms,
                result_message,
                result_duration_ms,
                created_at,
                started_at,
                finished_at
            FROM jobs
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn get(&self, id: JobId) -> Result<Option<JobRecord>, RepositoryError> {
        let row = sqlx::query_as::<_, JobRow>(
            r#"
            SELECT
                id,
                name,
                state,
                progress_completed,
                progress_total,
                delay_ms,
                result_message,
                result_duration_ms,
                created_at,
                started_at,
                finished_at
            FROM jobs
            WHERE id = $1
            "#,
        )
        .bind(encode_job_id(id)?)
        .fetch_optional(&self.pool)
        .await?;

        row.map(TryInto::try_into).transpose()
    }

    pub async fn claim_next(&self) -> Result<Option<ClaimedJob>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let candidate = sqlx::query_as::<_, ClaimRow>(
            r#"
            SELECT id, delay_ms
            FROM jobs
            WHERE state = 'queued'
              AND available_at <= CURRENT_TIMESTAMP
            ORDER BY available_at, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(candidate) = candidate else {
            transaction.commit().await?;
            return Ok(None);
        };
        let job_id = decode_job_id(candidate.id)?;
        let delay = decode_duration(candidate.delay_ms)?;

        let attempt_number = sqlx::query_scalar::<_, i32>(
            r#"
            SELECT COALESCE(MAX(attempt_number), 0) + 1
            FROM job_attempts
            WHERE job_id = $1
            "#,
        )
        .bind(candidate.id)
        .fetch_one(&mut *transaction)
        .await?;

        let updated = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'running',
                started_at = CURRENT_TIMESTAMP,
                progress_completed = 0,
                result_message = NULL,
                result_duration_ms = NULL,
                finished_at = NULL
            WHERE id = $1
              AND state = 'queued'
            "#,
        )
        .bind(candidate.id)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(updated.rows_affected(), "claim job")?;

        let attempt_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO job_attempts (job_id, attempt_number, state)
            VALUES ($1, $2, 'running')
            RETURNING id
            "#,
        )
        .bind(candidate.id)
        .bind(attempt_number)
        .fetch_one(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(Some(ClaimedJob {
            job_id,
            attempt_id,
            delay,
        }))
    }

    pub async fn complete(
        &self,
        claimed: &ClaimedJob,
        duration: Duration,
    ) -> Result<(), RepositoryError> {
        let duration_ms = i64::try_from(duration.as_millis()).map_err(|_| {
            RepositoryError::InvalidData("worker duration exceeds BIGINT range".into())
        })?;
        let job_id = encode_job_id(claimed.job_id)?;
        let mut transaction = self.pool.begin().await?;

        let attempt = sqlx::query(
            r#"
            UPDATE job_attempts
            SET state = 'succeeded',
                progress_completed = progress_total,
                finished_at = CURRENT_TIMESTAMP,
                duration_ms = $2
            WHERE id = $1
              AND job_id = $3
              AND state = 'running'
            "#,
        )
        .bind(claimed.attempt_id)
        .bind(duration_ms)
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(attempt.rows_affected(), "complete attempt")?;

        let job = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'succeeded',
                progress_completed = progress_total,
                result_message = $2,
                result_duration_ms = $3,
                finished_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND state = 'running'
            "#,
        )
        .bind(job_id)
        .bind(RESULT_MESSAGE)
        .bind(duration_ms)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(job.rows_affected(), "complete job")?;

        transaction.commit().await?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct JobRecord {
    job: Job,
    progress_completed: u32,
    progress_total: u32,
    delay_ms: u64,
    result_message: Option<String>,
    result_duration_ms: Option<u64>,
    created_at: OffsetDateTime,
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
}

impl JobRecord {
    pub const fn job(&self) -> &Job {
        &self.job
    }

    pub const fn progress_completed(&self) -> u32 {
        self.progress_completed
    }

    pub const fn progress_total(&self) -> u32 {
        self.progress_total
    }

    pub const fn delay_ms(&self) -> u64 {
        self.delay_ms
    }

    pub fn result_message(&self) -> Option<&str> {
        self.result_message.as_deref()
    }

    pub const fn result_duration_ms(&self) -> Option<u64> {
        self.result_duration_ms
    }

    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    pub const fn started_at(&self) -> Option<OffsetDateTime> {
        self.started_at
    }

    pub const fn finished_at(&self) -> Option<OffsetDateTime> {
        self.finished_at
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedJob {
    job_id: JobId,
    attempt_id: i64,
    delay: Duration,
}

impl ClaimedJob {
    pub const fn job_id(&self) -> JobId {
        self.job_id
    }

    pub const fn attempt_id(&self) -> i64 {
        self.attempt_id
    }

    pub const fn delay(&self) -> Duration {
        self.delay
    }
}

#[derive(Debug)]
pub enum RepositoryError {
    Database(sqlx::Error),
    Migration(MigrateError),
    InvalidData(String),
    StateConflict(&'static str),
}

impl Display for RepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database operation failed: {error}"),
            Self::Migration(error) => write!(formatter, "database migration failed: {error}"),
            Self::InvalidData(message) => {
                write!(formatter, "database returned invalid data: {message}")
            }
            Self::StateConflict(operation) => {
                write!(formatter, "job state changed while trying to {operation}")
            }
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::InvalidData(_) | Self::StateConflict(_) => None,
        }
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<MigrateError> for RepositoryError {
    fn from(error: MigrateError) -> Self {
        Self::Migration(error)
    }
}

#[derive(Debug, FromRow)]
struct JobRow {
    id: i64,
    name: String,
    state: String,
    progress_completed: i32,
    progress_total: i32,
    delay_ms: i32,
    result_message: Option<String>,
    result_duration_ms: Option<i64>,
    created_at: OffsetDateTime,
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
}

impl TryFrom<JobRow> for JobRecord {
    type Error = RepositoryError;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        let id = decode_job_id(row.id)?;
        let name = JobName::new(row.name)
            .map_err(|error| RepositoryError::InvalidData(error.to_string()))?;
        let status = row
            .state
            .parse::<JobStatus>()
            .map_err(|error| RepositoryError::InvalidData(error.to_string()))?;
        let progress_completed = decode_u32(row.progress_completed, "progress_completed")?;
        let progress_total = decode_u32(row.progress_total, "progress_total")?;

        if progress_total == 0 || progress_completed > progress_total {
            return Err(RepositoryError::InvalidData(
                "job progress violates its invariant".into(),
            ));
        }

        Ok(Self {
            job: Job::restore(id, name, status),
            progress_completed,
            progress_total,
            delay_ms: decode_u64(i64::from(row.delay_ms), "delay_ms")?,
            result_message: row.result_message,
            result_duration_ms: row
                .result_duration_ms
                .map(|value| decode_u64(value, "result_duration_ms"))
                .transpose()?,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct ClaimRow {
    id: i64,
    delay_ms: i32,
}

fn decode_job_id(value: i64) -> Result<JobId, RepositoryError> {
    let value = decode_u64(value, "job ID")?;
    JobId::new(value).map_err(|error| RepositoryError::InvalidData(error.to_string()))
}

fn encode_job_id(id: JobId) -> Result<i64, RepositoryError> {
    i64::try_from(id.get())
        .map_err(|_| RepositoryError::InvalidData("job ID exceeds BIGINT range".into()))
}

fn decode_duration(value: i32) -> Result<Duration, RepositoryError> {
    Ok(Duration::from_millis(decode_u64(
        i64::from(value),
        "delay_ms",
    )?))
}

fn decode_u32(value: i32, field: &str) -> Result<u32, RepositoryError> {
    u32::try_from(value)
        .map_err(|_| RepositoryError::InvalidData(format!("{field} cannot be negative")))
}

fn decode_u64(value: i64, field: &str) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::InvalidData(format!("{field} cannot be negative")))
}

fn ensure_one_row(rows_affected: u64, operation: &'static str) -> Result<(), RepositoryError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(RepositoryError::StateConflict(operation))
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::time::Duration;

    use sqlx::FromRow;
    use taskharbor_core::{JobName, JobStatus};

    use super::PgJobRepository;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
    async fn persists_claims_and_completes_jobs_in_queue_order() {
        let database_url = env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point to an isolated test database");
        let repository = PgJobRepository::connect(&database_url, 2)
            .await
            .expect("test database should be reachable");
        repository
            .migrate()
            .await
            .expect("migrations should succeed");
        sqlx::query("TRUNCATE job_attempts, jobs RESTART IDENTITY CASCADE")
            .execute(&repository.pool)
            .await
            .expect("test tables should be reset");

        let first = repository
            .create(JobName::new("first job").expect("test name should be valid"))
            .await
            .expect("first job should be created");
        let second = repository
            .create(JobName::new("second job").expect("test name should be valid"))
            .await
            .expect("second job should be created");

        let reconnected = PgJobRepository::connect(&database_url, 2)
            .await
            .expect("a second repository should connect");
        assert!(
            reconnected
                .get(first.job().id())
                .await
                .expect("persisted job should be readable")
                .is_some()
        );

        let claimed_first = repository
            .claim_next()
            .await
            .expect("claim should succeed")
            .expect("the first job should be claimable");
        assert_eq!(claimed_first.job_id(), first.job().id());
        repository
            .complete(&claimed_first, Duration::from_millis(12))
            .await
            .expect("first job should complete");

        let completed = repository
            .get(first.job().id())
            .await
            .expect("completed job should be readable")
            .expect("completed job should still exist");
        assert_eq!(completed.job().status(), JobStatus::Succeeded);
        assert_eq!(completed.progress_completed(), completed.progress_total());
        assert_eq!(completed.result_message(), Some("demo delay completed"));

        let attempt = sqlx::query_as::<_, AttemptCheck>(
            r#"
            SELECT state, progress_completed, progress_total, duration_ms
            FROM job_attempts
            WHERE job_id = $1
            "#,
        )
        .bind(i64::try_from(first.job().id().get()).expect("test ID should fit BIGINT"))
        .fetch_one(&repository.pool)
        .await
        .expect("attempt should be recorded");
        assert_eq!(attempt.state, "succeeded");
        assert_eq!(attempt.progress_completed, attempt.progress_total);
        assert_eq!(attempt.duration_ms, Some(12));

        let claimed_second = repository
            .claim_next()
            .await
            .expect("second claim should succeed")
            .expect("the second job should be claimable");
        assert_eq!(claimed_second.job_id(), second.job().id());
        repository
            .complete(&claimed_second, Duration::from_millis(8))
            .await
            .expect("second job should complete");

        reconnected.pool.close().await;
        assert!(reconnected.claim_next().await.is_err());
    }

    #[derive(Debug, FromRow)]
    struct AttemptCheck {
        state: String,
        progress_completed: i32,
        progress_total: i32,
        duration_ms: Option<i64>,
    }
}
