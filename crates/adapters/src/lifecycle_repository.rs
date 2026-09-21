use std::time::Duration;

use sqlx::{FromRow, PgPool};
use taskharbor_core::JobId;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth_repository::UserId;
use crate::postgres::{
    ClaimedJob, JobRecord, PgJobRepository, RepositoryError, decode_job_id, decode_u32, decode_u64,
    encode_job_id, ensure_claim_row, ensure_one_row,
};
use crate::worker_repository::WorkerId;

pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
pub const MAX_ATTEMPTS: u32 = 10;
const MAX_BACKOFF_SECONDS: u32 = 60;
const CANCELLED_MESSAGE: &str = "cancelled by user";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Transient,
    Permanent,
}

impl FailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Permanent => "permanent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl AttemptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttemptRecord {
    id: i64,
    attempt_number: u32,
    status: AttemptStatus,
    progress_completed: u32,
    progress_total: u32,
    started_at: OffsetDateTime,
    finished_at: Option<OffsetDateTime>,
    duration_ms: Option<u64>,
    error_kind: Option<String>,
    error_message: Option<String>,
    worker_id: Option<WorkerId>,
    worker_name: Option<String>,
    lease_expires_at: Option<OffsetDateTime>,
}

impl AttemptRecord {
    pub const fn id(&self) -> i64 {
        self.id
    }

    pub const fn attempt_number(&self) -> u32 {
        self.attempt_number
    }

    pub const fn status(&self) -> AttemptStatus {
        self.status
    }

    pub const fn progress_completed(&self) -> u32 {
        self.progress_completed
    }

    pub const fn progress_total(&self) -> u32 {
        self.progress_total
    }

    pub const fn started_at(&self) -> OffsetDateTime {
        self.started_at
    }

    pub const fn finished_at(&self) -> Option<OffsetDateTime> {
        self.finished_at
    }

    pub const fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    pub fn error_kind(&self) -> Option<&str> {
        self.error_kind.as_deref()
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub const fn worker_id(&self) -> Option<WorkerId> {
        self.worker_id
    }

    pub fn worker_name(&self) -> Option<&str> {
        self.worker_name.as_deref()
    }

    pub const fn lease_expires_at(&self) -> Option<OffsetDateTime> {
        self.lease_expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    RetryScheduled { available_at: OffsetDateTime },
    Failed,
}

impl PgJobRepository {
    pub async fn cancellation_requested(
        &self,
        claimed: &ClaimedJob,
    ) -> Result<bool, RepositoryError> {
        let state = sqlx::query_scalar::<_, String>(
            r#"
            SELECT job.state
            FROM jobs AS job
            INNER JOIN job_attempts AS attempt
                ON attempt.job_id = job.id
               AND attempt.id = $2
            WHERE job.id = $1
              AND attempt.state = 'running'
              AND attempt.worker_id = $3
              AND attempt.claim_token = $4
              AND attempt.lease_expires_at > CURRENT_TIMESTAMP
            "#,
        )
        .bind(encode_job_id(claimed.job_id())?)
        .bind(claimed.attempt_id())
        .bind(claimed.worker_id().as_uuid())
        .bind(claimed.claim_token())
        .fetch_optional(&self.pool)
        .await?;

        match state.as_deref() {
            Some("running") => Ok(false),
            Some("cancel_requested") => Ok(true),
            _ => Err(RepositoryError::ClaimLost),
        }
    }

    pub async fn finish_cancelled(
        &self,
        claimed: &ClaimedJob,
        duration: Duration,
    ) -> Result<(), RepositoryError> {
        let duration_ms = encode_duration(duration)?;
        let job_id = encode_job_id(claimed.job_id())?;
        let mut transaction = self.pool.begin().await?;

        let attempt = sqlx::query(
            r#"
            UPDATE job_attempts
            SET state = 'cancelled',
                finished_at = CURRENT_TIMESTAMP,
                duration_ms = $3,
                error_kind = 'cancelled',
                error_message = $4
            WHERE id = $1
              AND job_id = $2
              AND state = 'running'
              AND worker_id = $5
              AND claim_token = $6
              AND lease_expires_at > CURRENT_TIMESTAMP
            "#,
        )
        .bind(claimed.attempt_id())
        .bind(job_id)
        .bind(duration_ms)
        .bind(CANCELLED_MESSAGE)
        .bind(claimed.worker_id().as_uuid())
        .bind(claimed.claim_token())
        .execute(&mut *transaction)
        .await?;
        ensure_claim_row(attempt.rows_affected())?;

        let job = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'cancelled',
                result_message = NULL,
                result_duration_ms = NULL,
                failure_message = NULL,
                finished_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND state = 'cancel_requested'
            "#,
        )
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(job.rows_affected(), "cancel job")?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_failure(
        &self,
        claimed: &ClaimedJob,
        duration: Duration,
        kind: FailureKind,
        safe_message: &str,
    ) -> Result<FailureDisposition, RepositoryError> {
        let duration_ms = encode_duration(duration)?;
        let job_id = encode_job_id(claimed.job_id())?;
        let mut transaction = self.pool.begin().await?;

        let attempt = sqlx::query(
            r#"
            UPDATE job_attempts
            SET state = 'failed',
                finished_at = CURRENT_TIMESTAMP,
                duration_ms = $3,
                error_kind = $4,
                error_message = $5
            WHERE id = $1
              AND job_id = $2
              AND state = 'running'
              AND worker_id = $6
              AND claim_token = $7
              AND lease_expires_at > CURRENT_TIMESTAMP
            "#,
        )
        .bind(claimed.attempt_id())
        .bind(job_id)
        .bind(duration_ms)
        .bind(kind.as_str())
        .bind(safe_message)
        .bind(claimed.worker_id().as_uuid())
        .bind(claimed.claim_token())
        .execute(&mut *transaction)
        .await?;
        ensure_claim_row(attempt.rows_affected())?;

        let can_retry =
            kind == FailureKind::Transient && claimed.attempt_number() < claimed.max_attempts();
        let disposition = if can_retry {
            let backoff_seconds = retry_backoff_seconds(claimed.attempt_number());
            let available_at = sqlx::query_scalar::<_, OffsetDateTime>(
                r#"
                UPDATE jobs
                SET state = 'retry_waiting',
                    available_at = CURRENT_TIMESTAMP + ($2 * INTERVAL '1 second'),
                    progress_completed = 0,
                    failure_message = $3,
                    result_message = NULL,
                    result_duration_ms = NULL,
                    finished_at = NULL
                WHERE id = $1
                  AND state = 'running'
                RETURNING available_at
                "#,
            )
            .bind(job_id)
            .bind(i32::try_from(backoff_seconds).map_err(|_| {
                RepositoryError::InvalidData("retry backoff exceeds INTEGER".into())
            })?)
            .bind(safe_message)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(RepositoryError::StateConflict("schedule retry"))?;
            FailureDisposition::RetryScheduled { available_at }
        } else {
            let job = sqlx::query(
                r#"
                UPDATE jobs
                SET state = 'failed',
                    failure_message = $2,
                    result_message = NULL,
                    result_duration_ms = NULL,
                    finished_at = CURRENT_TIMESTAMP
                WHERE id = $1
                  AND state = 'running'
                "#,
            )
            .bind(job_id)
            .bind(safe_message)
            .execute(&mut *transaction)
            .await?;
            ensure_one_row(job.rows_affected(), "fail job")?;
            FailureDisposition::Failed
        };

        transaction.commit().await?;
        Ok(disposition)
    }

    pub async fn request_cancel(&self, id: JobId) -> Result<Option<JobRecord>, RepositoryError> {
        self.request_cancel_with_owner(id, None).await
    }

    pub async fn request_cancel_for_owner(
        &self,
        id: JobId,
        owner_user_id: UserId,
    ) -> Result<Option<JobRecord>, RepositoryError> {
        self.request_cancel_with_owner(id, Some(owner_user_id))
            .await
    }

    async fn request_cancel_with_owner(
        &self,
        id: JobId,
        owner_user_id: Option<UserId>,
    ) -> Result<Option<JobRecord>, RepositoryError> {
        let job_id = encode_job_id(id)?;
        let updated = sqlx::query_scalar::<_, String>(
            r#"
            UPDATE jobs
            SET state = CASE
                    WHEN state IN ('queued', 'retry_waiting') THEN 'cancelled'
                    WHEN state = 'running' THEN 'cancel_requested'
                    ELSE state
                END,
                cancel_requested_at = COALESCE(cancel_requested_at, CURRENT_TIMESTAMP),
                finished_at = CASE
                    WHEN state IN ('queued', 'retry_waiting') THEN CURRENT_TIMESTAMP
                    ELSE finished_at
                END,
                failure_message = CASE
                    WHEN state IN ('queued', 'retry_waiting') THEN NULL
                    ELSE failure_message
                END
            WHERE id = $1
              AND ($2::BIGINT IS NULL OR owner_user_id = $2)
              AND state IN (
                  'queued',
                  'retry_waiting',
                  'running',
                  'cancel_requested',
                  'cancelled'
              )
            RETURNING state
            "#,
        )
        .bind(job_id)
        .bind(owner_user_id.map(UserId::get))
        .fetch_optional(&self.pool)
        .await?;

        if updated.is_none() {
            let exists = match owner_user_id {
                Some(owner_user_id) => self.get_for_owner(id, owner_user_id).await?.is_some(),
                None => self.get(id).await?.is_some(),
            };
            return if exists {
                Err(RepositoryError::StateConflict("cancel terminal job"))
            } else {
                Ok(None)
            };
        }

        match owner_user_id {
            Some(owner_user_id) => self.get_for_owner(id, owner_user_id).await,
            None => self.get(id).await,
        }
    }

    pub async fn manual_retry(&self, id: JobId) -> Result<Option<JobRecord>, RepositoryError> {
        self.manual_retry_with_owner(id, None).await
    }

    pub async fn manual_retry_for_owner(
        &self,
        id: JobId,
        owner_user_id: UserId,
    ) -> Result<Option<JobRecord>, RepositoryError> {
        self.manual_retry_with_owner(id, Some(owner_user_id)).await
    }

    async fn manual_retry_with_owner(
        &self,
        id: JobId,
        owner_user_id: Option<UserId>,
    ) -> Result<Option<JobRecord>, RepositoryError> {
        let source_id = encode_job_id(id)?;
        let mut transaction = self.pool.begin().await?;
        let source_state = sqlx::query_scalar::<_, String>(
            r#"
            SELECT state
            FROM jobs
            WHERE id = $1
              AND ($2::BIGINT IS NULL OR owner_user_id = $2)
            FOR UPDATE
            "#,
        )
        .bind(source_id)
        .bind(owner_user_id.map(UserId::get))
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(source_state) = source_state else {
            transaction.rollback().await?;
            return Ok(None);
        };
        if !matches!(source_state.as_str(), "failed" | "cancelled") {
            return Err(RepositoryError::StateConflict("manually retry job"));
        }

        let new_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO jobs (
                name,
                job_type,
                state,
                available_at,
                progress_total,
                delay_ms,
                max_width,
                jpeg_quality,
                max_attempts,
                priority,
                retry_of_job_id,
                owner_user_id
            )
            SELECT
                name,
                job_type,
                'queued',
                CURRENT_TIMESTAMP,
                progress_total,
                delay_ms,
                max_width,
                jpeg_quality,
                max_attempts,
                priority,
                id,
                owner_user_id
            FROM jobs
            WHERE id = $1
            RETURNING id
            "#,
        )
        .bind(source_id)
        .fetch_one(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO artifacts (
                job_id,
                kind,
                item_index,
                storage_key,
                display_name,
                media_type,
                byte_size,
                width,
                height
            )
            SELECT
                $2,
                kind,
                item_index,
                storage_key,
                display_name,
                media_type,
                byte_size,
                width,
                height
            FROM artifacts
            WHERE job_id = $1
              AND kind = 'input'
            "#,
        )
        .bind(source_id)
        .bind(new_id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        let new_id = decode_job_id(new_id)?;
        match owner_user_id {
            Some(owner_user_id) => self.get_for_owner(new_id, owner_user_id).await,
            None => self.get(new_id).await,
        }
    }
}

pub(crate) async fn load_attempts(
    pool: &PgPool,
    job_id: i64,
) -> Result<Vec<AttemptRecord>, RepositoryError> {
    sqlx::query_as::<_, AttemptRow>(
        r#"
        SELECT
            attempt.id,
            attempt.attempt_number,
            attempt.state,
            attempt.progress_completed,
            attempt.progress_total,
            attempt.started_at,
            attempt.finished_at,
            attempt.duration_ms,
            attempt.error_kind,
            attempt.error_message,
            attempt.worker_id,
            worker.name AS worker_name,
            attempt.lease_expires_at
        FROM job_attempts AS attempt
        LEFT JOIN workers AS worker ON worker.id = attempt.worker_id
        WHERE attempt.job_id = $1
        ORDER BY attempt.attempt_number
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) fn retry_backoff_seconds(attempt_number: u32) -> u32 {
    let shift = attempt_number.saturating_sub(1).min(31);
    1_u32
        .checked_shl(shift)
        .unwrap_or(u32::MAX)
        .min(MAX_BACKOFF_SECONDS)
}

fn encode_duration(value: Duration) -> Result<i64, RepositoryError> {
    i64::try_from(value.as_millis())
        .map_err(|_| RepositoryError::InvalidData("worker duration exceeds BIGINT".into()))
}

#[derive(Debug, FromRow)]
struct AttemptRow {
    id: i64,
    attempt_number: i32,
    state: String,
    progress_completed: i32,
    progress_total: i32,
    started_at: OffsetDateTime,
    finished_at: Option<OffsetDateTime>,
    duration_ms: Option<i64>,
    error_kind: Option<String>,
    error_message: Option<String>,
    worker_id: Option<Uuid>,
    worker_name: Option<String>,
    lease_expires_at: Option<OffsetDateTime>,
}

impl TryFrom<AttemptRow> for AttemptRecord {
    type Error = RepositoryError;

    fn try_from(row: AttemptRow) -> Result<Self, Self::Error> {
        let status = match row.state.as_str() {
            "running" => AttemptStatus::Running,
            "succeeded" => AttemptStatus::Succeeded,
            "failed" => AttemptStatus::Failed,
            "cancelled" => AttemptStatus::Cancelled,
            _ => {
                return Err(RepositoryError::InvalidData(
                    "attempt state is not recognized".into(),
                ));
            }
        };
        let progress_completed = decode_u32(row.progress_completed, "attempt progress completed")?;
        let progress_total = decode_u32(row.progress_total, "attempt progress total")?;
        if progress_total == 0 || progress_completed > progress_total {
            return Err(RepositoryError::InvalidData(
                "attempt progress violates its invariant".into(),
            ));
        }

        Ok(Self {
            id: row.id,
            attempt_number: decode_u32(row.attempt_number, "attempt number")?,
            status,
            progress_completed,
            progress_total,
            started_at: row.started_at,
            finished_at: row.finished_at,
            duration_ms: row
                .duration_ms
                .map(|value| decode_u64(value, "attempt duration"))
                .transpose()?,
            error_kind: row.error_kind,
            error_message: row.error_message,
            worker_id: row.worker_id.map(WorkerId::from_uuid),
            worker_name: row.worker_name,
            lease_expires_at: row.lease_expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::retry_backoff_seconds;

    #[test]
    fn retry_backoff_is_exponential_and_capped() {
        assert_eq!(retry_backoff_seconds(1), 1);
        assert_eq!(retry_backoff_seconds(2), 2);
        assert_eq!(retry_backoff_seconds(3), 4);
        assert_eq!(retry_backoff_seconds(10), 60);
    }
}
