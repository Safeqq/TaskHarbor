use std::collections::HashSet;
use std::time::Duration;

use sqlx::Row;

use crate::auth_repository::UserId;
use crate::postgres::{PgJobRepository, RepositoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueMetrics {
    pub depth: u64,
    pub oldest_wait_ms: u64,
}

impl PgJobRepository {
    pub async fn active_job_count_for_owner(
        &self,
        owner_user_id: UserId,
    ) -> Result<u64, RepositoryError> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM jobs
            WHERE owner_user_id = $1
              AND state IN ('queued', 'running', 'retry_waiting', 'cancel_requested')
            "#,
        )
        .bind(owner_user_id.get())
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count)
            .map_err(|_| RepositoryError::InvalidData("active job count is negative".into()))
    }

    pub async fn queue_metrics(&self) -> Result<QueueMetrics, RepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) AS depth,
                COALESCE(
                    EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - MIN(available_at))) * 1000,
                    0
                )::BIGINT AS oldest_wait_ms
            FROM jobs
            WHERE state IN ('queued', 'retry_waiting')
              AND available_at <= CURRENT_TIMESTAMP
            "#,
        )
        .fetch_one(&self.pool)
        .await?;
        let depth: i64 = row.try_get("depth")?;
        let oldest_wait_ms: i64 = row.try_get("oldest_wait_ms")?;
        Ok(QueueMetrics {
            depth: u64::try_from(depth)
                .map_err(|_| RepositoryError::InvalidData("queue depth is negative".into()))?,
            oldest_wait_ms: u64::try_from(oldest_wait_ms).map_err(|_| {
                RepositoryError::InvalidData("oldest queue wait is negative".into())
            })?,
        })
    }

    pub async fn storage_references(&self) -> Result<HashSet<String>, RepositoryError> {
        let keys = sqlx::query_scalar::<_, String>(
            r#"
            SELECT storage_key FROM artifacts
            UNION
            SELECT storage_key FROM schedule_inputs
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(keys.into_iter().collect())
    }

    pub async fn active_attempt_output_prefixes(&self) -> Result<Vec<String>, RepositoryError> {
        let attempts = sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT job_id, id
            FROM job_attempts
            WHERE state = 'running'
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        attempts
            .into_iter()
            .map(|(job_id, attempt_id)| {
                let job_id = u64::try_from(job_id).map_err(|_| {
                    RepositoryError::InvalidData("active attempt has an invalid job ID".into())
                })?;
                Ok(format!("outputs/job-{job_id}/attempt-{attempt_id}"))
            })
            .collect()
    }

    pub async fn expire_output_artifacts(
        &self,
        retention: Duration,
    ) -> Result<Vec<String>, RepositoryError> {
        let retention_seconds = i64::try_from(retention.as_secs())
            .map_err(|_| RepositoryError::InvalidData("output retention exceeds BIGINT".into()))?;
        let mut transaction = self.pool.begin().await?;
        let keys = sqlx::query_scalar::<_, String>(
            r#"
            DELETE FROM artifacts AS artifact
            USING jobs AS job
            WHERE artifact.job_id = job.id
              AND artifact.kind = 'output'
              AND job.state IN ('succeeded', 'failed', 'cancelled')
              AND job.finished_at <= CURRENT_TIMESTAMP - ($1 * INTERVAL '1 second')
            RETURNING artifact.storage_key
            "#,
        )
        .bind(retention_seconds)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE jobs
            SET outputs_expired_at = CURRENT_TIMESTAMP
            WHERE state IN ('succeeded', 'failed', 'cancelled')
              AND finished_at <= CURRENT_TIMESTAMP - ($1 * INTERVAL '1 second')
              AND outputs_expired_at IS NULL
            "#,
        )
        .bind(retention_seconds)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(keys)
    }
}
