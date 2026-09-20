use std::fmt::{self, Display, Formatter};
use std::str::FromStr;
use std::time::Duration;

use sqlx::{FromRow, Postgres, Transaction};
use taskharbor_core::JobId;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::lifecycle_repository::retry_backoff_seconds;
use crate::postgres::{
    ClaimedJob, PgJobRepository, RepositoryError, decode_job_id, decode_u32, encode_job_id,
    ensure_one_row,
};

pub const MIN_WORKER_CONCURRENCY: usize = 1;
pub const MAX_WORKER_CONCURRENCY: usize = 64;
pub const MIN_LEASE_DURATION: Duration = Duration::from_secs(2);
pub const MAX_LEASE_DURATION: Duration = Duration::from_secs(3_600);
const LEASE_EXPIRED_MESSAGE: &str = "worker lease expired before completion";
const LEASE_EXPIRED_CANCEL_MESSAGE: &str = "cancelled after worker lease expired";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerId(Uuid);

impl WorkerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for WorkerId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for WorkerId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl FromStr for WorkerId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Debug, Clone)]
pub struct WorkerRegistration {
    pub id: WorkerId,
    pub name: String,
    pub concurrency_limit: usize,
    pub lease_duration: Duration,
    pub heartbeat_ttl: Duration,
}

impl WorkerRegistration {
    fn validate(&self) -> Result<(), RepositoryError> {
        let name_length = self.name.trim().chars().count();
        if !(1..=100).contains(&name_length) {
            return Err(RepositoryError::InvalidData(
                "worker name must contain between 1 and 100 characters".into(),
            ));
        }
        if !(MIN_WORKER_CONCURRENCY..=MAX_WORKER_CONCURRENCY).contains(&self.concurrency_limit) {
            return Err(RepositoryError::InvalidData(format!(
                "worker concurrency must be between {MIN_WORKER_CONCURRENCY} and {MAX_WORKER_CONCURRENCY}"
            )));
        }
        validate_duration(self.lease_duration, "lease duration")?;
        validate_duration(self.heartbeat_ttl, "heartbeat TTL")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStatus {
    Online,
    Offline,
    Stopped,
}

impl WorkerStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerRecord {
    id: WorkerId,
    name: String,
    status: WorkerStatus,
    concurrency_limit: u16,
    lease_duration_seconds: u32,
    active_attempts: u32,
    started_at: OffsetDateTime,
    last_heartbeat_at: OffsetDateTime,
    heartbeat_expires_at: OffsetDateTime,
    stopped_at: Option<OffsetDateTime>,
}

impl WorkerRecord {
    pub const fn id(&self) -> WorkerId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn status(&self) -> WorkerStatus {
        self.status
    }

    pub const fn concurrency_limit(&self) -> u16 {
        self.concurrency_limit
    }

    pub const fn lease_duration_seconds(&self) -> u32 {
        self.lease_duration_seconds
    }

    pub const fn active_attempts(&self) -> u32 {
        self.active_attempts
    }

    pub const fn started_at(&self) -> OffsetDateTime {
        self.started_at
    }

    pub const fn last_heartbeat_at(&self) -> OffsetDateTime {
        self.last_heartbeat_at
    }

    pub const fn heartbeat_expires_at(&self) -> OffsetDateTime {
        self.heartbeat_expires_at
    }

    pub const fn stopped_at(&self) -> Option<OffsetDateTime> {
        self.stopped_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimDisposition {
    RetryScheduled { available_at: OffsetDateTime },
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
pub struct ReclaimedAttempt {
    job_id: JobId,
    attempt_id: i64,
    worker_id: WorkerId,
    disposition: ReclaimDisposition,
}

impl ReclaimedAttempt {
    pub const fn job_id(self) -> JobId {
        self.job_id
    }

    pub const fn attempt_id(self) -> i64 {
        self.attempt_id
    }

    pub const fn worker_id(self) -> WorkerId {
        self.worker_id
    }

    pub const fn disposition(self) -> ReclaimDisposition {
        self.disposition
    }
}

impl PgJobRepository {
    pub async fn register_worker(
        &self,
        registration: &WorkerRegistration,
    ) -> Result<WorkerRecord, RepositoryError> {
        let as_of = self.database_now().await?;
        self.register_worker_at(registration, as_of).await
    }

    pub async fn register_worker_at(
        &self,
        registration: &WorkerRegistration,
        as_of: OffsetDateTime,
    ) -> Result<WorkerRecord, RepositoryError> {
        registration.validate()?;
        let concurrency_limit = i16::try_from(registration.concurrency_limit).map_err(|_| {
            RepositoryError::InvalidData("worker concurrency exceeds SMALLINT".into())
        })?;
        let lease_seconds = duration_seconds(registration.lease_duration, "lease duration")?;
        let heartbeat_seconds = duration_seconds(registration.heartbeat_ttl, "heartbeat TTL")?;

        sqlx::query(
            r#"
            INSERT INTO workers (
                id,
                name,
                concurrency_limit,
                lease_duration_seconds,
                started_at,
                last_heartbeat_at,
                heartbeat_expires_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $5,
                $5 + ($6 * INTERVAL '1 second')
            )
            "#,
        )
        .bind(registration.id.as_uuid())
        .bind(registration.name.trim())
        .bind(concurrency_limit)
        .bind(lease_seconds)
        .bind(as_of)
        .bind(heartbeat_seconds)
        .execute(&self.pool)
        .await?;

        self.get_worker(registration.id)
            .await?
            .ok_or(RepositoryError::StateConflict("read registered worker"))
    }

    pub async fn heartbeat_worker(
        &self,
        worker_id: WorkerId,
        heartbeat_ttl: Duration,
    ) -> Result<OffsetDateTime, RepositoryError> {
        let as_of = self.database_now().await?;
        self.heartbeat_worker_at(worker_id, heartbeat_ttl, as_of)
            .await
    }

    pub async fn heartbeat_worker_at(
        &self,
        worker_id: WorkerId,
        heartbeat_ttl: Duration,
        as_of: OffsetDateTime,
    ) -> Result<OffsetDateTime, RepositoryError> {
        validate_duration(heartbeat_ttl, "heartbeat TTL")?;
        let heartbeat_seconds = duration_seconds(heartbeat_ttl, "heartbeat TTL")?;
        sqlx::query_scalar::<_, OffsetDateTime>(
            r#"
            UPDATE workers
            SET last_heartbeat_at = $2,
                heartbeat_expires_at = $2 + ($3 * INTERVAL '1 second')
            WHERE id = $1
              AND stopped_at IS NULL
            RETURNING heartbeat_expires_at
            "#,
        )
        .bind(worker_id.as_uuid())
        .bind(as_of)
        .bind(heartbeat_seconds)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::StateConflict("heartbeat worker"))
    }

    pub async fn stop_worker(&self, worker_id: WorkerId) -> Result<(), RepositoryError> {
        let stopped = sqlx::query(
            r#"
            UPDATE workers
            SET stopped_at = COALESCE(stopped_at, CURRENT_TIMESTAMP)
            WHERE id = $1
            "#,
        )
        .bind(worker_id.as_uuid())
        .execute(&self.pool)
        .await?;
        ensure_one_row(stopped.rows_affected(), "stop worker")
    }

    pub async fn get_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Option<WorkerRecord>, RepositoryError> {
        let row = sqlx::query_as::<_, WorkerRow>(
            r#"
            SELECT
                worker.id,
                worker.name,
                CASE
                    WHEN worker.stopped_at IS NOT NULL THEN 'stopped'
                    WHEN worker.heartbeat_expires_at > CURRENT_TIMESTAMP THEN 'online'
                    ELSE 'offline'
                END AS status,
                worker.concurrency_limit,
                worker.lease_duration_seconds,
                (
                    SELECT COUNT(*)
                    FROM job_attempts AS attempt
                    WHERE attempt.worker_id = worker.id
                      AND attempt.state = 'running'
                ) AS active_attempts,
                worker.started_at,
                worker.last_heartbeat_at,
                worker.heartbeat_expires_at,
                worker.stopped_at
            FROM workers AS worker
            WHERE worker.id = $1
            "#,
        )
        .bind(worker_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn list_workers(&self) -> Result<Vec<WorkerRecord>, RepositoryError> {
        sqlx::query_as::<_, WorkerRow>(
            r#"
            SELECT
                worker.id,
                worker.name,
                CASE
                    WHEN worker.stopped_at IS NOT NULL THEN 'stopped'
                    WHEN worker.heartbeat_expires_at > CURRENT_TIMESTAMP THEN 'online'
                    ELSE 'offline'
                END AS status,
                worker.concurrency_limit,
                worker.lease_duration_seconds,
                (
                    SELECT COUNT(*)
                    FROM job_attempts AS attempt
                    WHERE attempt.worker_id = worker.id
                      AND attempt.state = 'running'
                ) AS active_attempts,
                worker.started_at,
                worker.last_heartbeat_at,
                worker.heartbeat_expires_at,
                worker.stopped_at
            FROM workers AS worker
            ORDER BY worker.started_at DESC, worker.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
    }

    pub async fn renew_lease(
        &self,
        claimed: &ClaimedJob,
    ) -> Result<OffsetDateTime, RepositoryError> {
        self.renew_lease_with_time(claimed, None).await
    }

    pub async fn renew_lease_at(
        &self,
        claimed: &ClaimedJob,
        as_of: OffsetDateTime,
    ) -> Result<OffsetDateTime, RepositoryError> {
        self.renew_lease_with_time(claimed, Some(as_of)).await
    }

    async fn renew_lease_with_time(
        &self,
        claimed: &ClaimedJob,
        as_of: Option<OffsetDateTime>,
    ) -> Result<OffsetDateTime, RepositoryError> {
        sqlx::query_scalar::<_, OffsetDateTime>(
            r#"
            UPDATE job_attempts AS attempt
            SET lease_expires_at = COALESCE($5, CURRENT_TIMESTAMP)
                + (worker.lease_duration_seconds * INTERVAL '1 second')
            FROM workers AS worker, jobs AS job
            WHERE attempt.id = $1
              AND attempt.job_id = $2
              AND attempt.worker_id = $3
              AND attempt.claim_token = $4
              AND attempt.state = 'running'
              AND attempt.lease_expires_at > COALESCE($5, CURRENT_TIMESTAMP)
              AND worker.id = attempt.worker_id
              AND worker.stopped_at IS NULL
              AND worker.heartbeat_expires_at > COALESCE($5, CURRENT_TIMESTAMP)
              AND job.id = attempt.job_id
              AND job.state IN ('running', 'cancel_requested')
            RETURNING attempt.lease_expires_at
            "#,
        )
        .bind(claimed.attempt_id())
        .bind(encode_job_id(claimed.job_id())?)
        .bind(claimed.worker_id().as_uuid())
        .bind(claimed.claim_token())
        .bind(as_of)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::ClaimLost)
    }

    pub async fn reclaim_next_expired_attempt(
        &self,
    ) -> Result<Option<ReclaimedAttempt>, RepositoryError> {
        let as_of = self.database_now().await?;
        self.reclaim_next_expired_attempt_at(as_of).await
    }

    pub async fn reclaim_next_expired_attempt_at(
        &self,
        as_of: OffsetDateTime,
    ) -> Result<Option<ReclaimedAttempt>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let candidate = sqlx::query_as::<_, ExpiredAttemptRow>(
            r#"
            SELECT
                attempt.id AS attempt_id,
                attempt.job_id,
                attempt.worker_id,
                attempt.attempt_number,
                attempt.started_at,
                job.max_attempts,
                job.state AS job_state
            FROM job_attempts AS attempt
            INNER JOIN jobs AS job ON job.id = attempt.job_id
            WHERE attempt.state = 'running'
              AND attempt.lease_expires_at <= $1
              AND job.state IN ('running', 'cancel_requested')
            ORDER BY attempt.lease_expires_at, attempt.id
            FOR UPDATE OF attempt, job SKIP LOCKED
            LIMIT 1
            "#,
        )
        .bind(as_of)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(candidate) = candidate else {
            transaction.commit().await?;
            return Ok(None);
        };
        let worker_id = candidate.worker_id.ok_or_else(|| {
            RepositoryError::InvalidData("running attempt has no worker owner".into())
        })?;
        let duration_ms = elapsed_milliseconds(candidate.started_at, as_of)?;
        let cancelling = candidate.job_state == "cancel_requested";
        let attempt_state = if cancelling { "cancelled" } else { "failed" };
        let error_kind = if cancelling { "cancelled" } else { "transient" };
        let error_message = if cancelling {
            LEASE_EXPIRED_CANCEL_MESSAGE
        } else {
            LEASE_EXPIRED_MESSAGE
        };

        let expired = sqlx::query(
            r#"
            UPDATE job_attempts
            SET state = $3,
                finished_at = $4,
                duration_ms = $5,
                error_kind = $6,
                error_message = $7
            WHERE id = $1
              AND job_id = $2
              AND state = 'running'
              AND lease_expires_at <= $4
            "#,
        )
        .bind(candidate.attempt_id)
        .bind(candidate.job_id)
        .bind(attempt_state)
        .bind(as_of)
        .bind(duration_ms)
        .bind(error_kind)
        .bind(error_message)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(expired.rows_affected(), "expire worker attempt")?;

        let disposition = if cancelling {
            finish_reclaimed_cancel(&mut transaction, candidate.job_id, as_of).await?;
            ReclaimDisposition::Cancelled
        } else if candidate.attempt_number < candidate.max_attempts {
            let backoff_seconds =
                retry_backoff_seconds(decode_u32(candidate.attempt_number, "attempt number")?);
            let available_at = as_of
                .checked_add(time::Duration::seconds(i64::from(backoff_seconds)))
                .ok_or_else(|| RepositoryError::InvalidData("retry time overflowed".into()))?;
            schedule_reclaimed_retry(&mut transaction, candidate.job_id, as_of, available_at)
                .await?;
            ReclaimDisposition::RetryScheduled { available_at }
        } else {
            fail_reclaimed_job(&mut transaction, candidate.job_id, as_of).await?;
            ReclaimDisposition::Failed
        };

        transaction.commit().await?;
        Ok(Some(ReclaimedAttempt {
            job_id: decode_job_id(candidate.job_id)?,
            attempt_id: candidate.attempt_id,
            worker_id: WorkerId::from_uuid(worker_id),
            disposition,
        }))
    }

    pub(crate) async fn database_now(&self) -> Result<OffsetDateTime, RepositoryError> {
        Ok(sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&self.pool)
            .await?)
    }
}

async fn finish_reclaimed_cancel(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    as_of: OffsetDateTime,
) -> Result<(), RepositoryError> {
    let updated = sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'cancelled',
            result_message = NULL,
            result_duration_ms = NULL,
            failure_message = NULL,
            finished_at = $2
        WHERE id = $1
          AND state = 'cancel_requested'
        "#,
    )
    .bind(job_id)
    .bind(as_of)
    .execute(&mut **transaction)
    .await?;
    ensure_one_row(updated.rows_affected(), "cancel expired job")
}

async fn schedule_reclaimed_retry(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    as_of: OffsetDateTime,
    available_at: OffsetDateTime,
) -> Result<(), RepositoryError> {
    let updated = sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'retry_waiting',
            available_at = $2,
            progress_completed = 0,
            failure_message = $3,
            result_message = NULL,
            result_duration_ms = NULL,
            finished_at = NULL
        WHERE id = $1
          AND state = 'running'
          AND $4 <= $2
        "#,
    )
    .bind(job_id)
    .bind(available_at)
    .bind(LEASE_EXPIRED_MESSAGE)
    .bind(as_of)
    .execute(&mut **transaction)
    .await?;
    ensure_one_row(updated.rows_affected(), "retry expired job")
}

async fn fail_reclaimed_job(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    as_of: OffsetDateTime,
) -> Result<(), RepositoryError> {
    let updated = sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'failed',
            failure_message = $2,
            result_message = NULL,
            result_duration_ms = NULL,
            finished_at = $3
        WHERE id = $1
          AND state = 'running'
        "#,
    )
    .bind(job_id)
    .bind(LEASE_EXPIRED_MESSAGE)
    .bind(as_of)
    .execute(&mut **transaction)
    .await?;
    ensure_one_row(updated.rows_affected(), "fail expired job")
}

fn validate_duration(value: Duration, field: &str) -> Result<(), RepositoryError> {
    if value < MIN_LEASE_DURATION || value > MAX_LEASE_DURATION || value.subsec_nanos() != 0 {
        return Err(RepositoryError::InvalidData(format!(
            "{field} must be a whole number of seconds between {} and {}",
            MIN_LEASE_DURATION.as_secs(),
            MAX_LEASE_DURATION.as_secs()
        )));
    }
    Ok(())
}

fn duration_seconds(value: Duration, field: &str) -> Result<i32, RepositoryError> {
    i32::try_from(value.as_secs())
        .map_err(|_| RepositoryError::InvalidData(format!("{field} exceeds INTEGER")))
}

fn elapsed_milliseconds(
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
) -> Result<i64, RepositoryError> {
    let value = (finished_at - started_at).whole_milliseconds().max(0);
    i64::try_from(value)
        .map_err(|_| RepositoryError::InvalidData("attempt duration exceeds BIGINT".into()))
}

#[derive(Debug, FromRow)]
struct WorkerRow {
    id: Uuid,
    name: String,
    status: String,
    concurrency_limit: i16,
    lease_duration_seconds: i32,
    active_attempts: i64,
    started_at: OffsetDateTime,
    last_heartbeat_at: OffsetDateTime,
    heartbeat_expires_at: OffsetDateTime,
    stopped_at: Option<OffsetDateTime>,
}

impl TryFrom<WorkerRow> for WorkerRecord {
    type Error = RepositoryError;

    fn try_from(row: WorkerRow) -> Result<Self, Self::Error> {
        let status = match row.status.as_str() {
            "online" => WorkerStatus::Online,
            "offline" => WorkerStatus::Offline,
            "stopped" => WorkerStatus::Stopped,
            _ => {
                return Err(RepositoryError::InvalidData(
                    "worker status is not recognized".into(),
                ));
            }
        };
        Ok(Self {
            id: WorkerId::from_uuid(row.id),
            name: row.name,
            status,
            concurrency_limit: u16::try_from(row.concurrency_limit)
                .map_err(|_| RepositoryError::InvalidData("invalid worker concurrency".into()))?,
            lease_duration_seconds: decode_u32(
                row.lease_duration_seconds,
                "worker lease duration",
            )?,
            active_attempts: decode_u32(
                i32::try_from(row.active_attempts).map_err(|_| {
                    RepositoryError::InvalidData(
                        "worker active attempt count exceeds INTEGER".into(),
                    )
                })?,
                "worker active attempts",
            )?,
            started_at: row.started_at,
            last_heartbeat_at: row.last_heartbeat_at,
            heartbeat_expires_at: row.heartbeat_expires_at,
            stopped_at: row.stopped_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct ExpiredAttemptRow {
    attempt_id: i64,
    job_id: i64,
    worker_id: Option<Uuid>,
    attempt_number: i32,
    started_at: OffsetDateTime,
    max_attempts: i32,
    job_state: String,
}

#[cfg(test)]
mod tests {
    use super::{MAX_LEASE_DURATION, MIN_LEASE_DURATION, WorkerId, WorkerRegistration};

    #[test]
    fn worker_registration_rejects_invalid_limits() {
        let valid = WorkerRegistration {
            id: WorkerId::new(),
            name: "worker-a".into(),
            concurrency_limit: 2,
            lease_duration: MIN_LEASE_DURATION,
            heartbeat_ttl: MAX_LEASE_DURATION,
        };
        assert!(valid.validate().is_ok());

        let invalid = WorkerRegistration {
            name: " ".into(),
            concurrency_limit: 0,
            ..valid
        };
        assert!(invalid.validate().is_err());
    }
}
