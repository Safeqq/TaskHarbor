use std::time::Duration;

use sqlx::{FromRow, PgPool, Postgres, Transaction};
use taskharbor_core::{JobId, JobName, JobPriority};
use time::OffsetDateTime;

use crate::image_processing::{MAX_FILES_PER_JOB, MAX_OUTPUT_WIDTH};
use crate::postgres::{
    ClaimedJob, ClaimedWork, JobRecord, PgJobRepository, RepositoryError, decode_job_id,
    decode_u32, decode_u64, encode_job_id, ensure_claim_row, ensure_one_row,
};

#[derive(Debug)]
pub struct NewImageJob {
    pub name: JobName,
    pub max_width: u32,
    pub jpeg_quality: u8,
    pub available_at: Option<OffsetDateTime>,
    pub priority: JobPriority,
    pub inputs: Vec<NewInputArtifact>,
}

impl NewImageJob {
    fn validate(&self) -> Result<(), RepositoryError> {
        if self.inputs.is_empty() || self.inputs.len() > MAX_FILES_PER_JOB {
            return Err(RepositoryError::InvalidData(format!(
                "image job must contain between 1 and {MAX_FILES_PER_JOB} inputs"
            )));
        }
        if self.max_width == 0
            || self.max_width > MAX_OUTPUT_WIDTH
            || !(1..=100).contains(&self.jpeg_quality)
        {
            return Err(RepositoryError::InvalidData(
                "image settings are outside the supported range".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct NewInputArtifact {
    pub storage_key: String,
    pub display_name: String,
    pub media_type: String,
    pub byte_size: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct PendingOutputArtifact {
    pub source_artifact_id: u64,
    pub item_index: u16,
    pub storage_key: String,
    pub display_name: String,
    pub media_type: String,
    pub byte_size: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSettings {
    DemoDelay { delay_ms: u64 },
    ImageResize { max_width: u32, jpeg_quality: u8 },
}

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    id: u64,
    job_id: JobId,
    attempt_id: Option<i64>,
    source_artifact_id: Option<u64>,
    kind: ArtifactKind,
    item_index: u16,
    storage_key: String,
    display_name: String,
    media_type: String,
    byte_size: u64,
    width: u32,
    height: u32,
    created_at: OffsetDateTime,
}

impl ArtifactRecord {
    pub const fn id(&self) -> u64 {
        self.id
    }

    pub const fn job_id(&self) -> JobId {
        self.job_id
    }

    pub const fn attempt_id(&self) -> Option<i64> {
        self.attempt_id
    }

    pub const fn source_artifact_id(&self) -> Option<u64> {
        self.source_artifact_id
    }

    pub const fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub const fn item_index(&self) -> u16 {
        self.item_index
    }

    pub fn storage_key(&self) -> &str {
        &self.storage_key
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Input,
    Output,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

impl PgJobRepository {
    pub async fn create_image_job(
        &self,
        request: NewImageJob,
    ) -> Result<JobRecord, RepositoryError> {
        request.validate()?;
        let mut transaction = self.pool.begin().await?;
        let progress_total = i32::try_from(request.inputs.len())
            .map_err(|_| RepositoryError::InvalidData("too many image inputs".into()))?;
        let max_width = i32::try_from(request.max_width)
            .map_err(|_| RepositoryError::InvalidData("max width exceeds INTEGER".into()))?;

        let job_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO jobs (
                name,
                job_type,
                available_at,
                priority,
                progress_total,
                max_width,
                jpeg_quality
            )
            VALUES ($1, 'image_resize', COALESCE($2, CURRENT_TIMESTAMP), $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(request.name.as_str())
        .bind(request.available_at)
        .bind(request.priority.as_str())
        .bind(progress_total)
        .bind(max_width)
        .bind(i16::from(request.jpeg_quality))
        .fetch_one(&mut *transaction)
        .await?;

        for (item_index, input) in request.inputs.into_iter().enumerate() {
            insert_input_artifact(
                &mut transaction,
                job_id,
                i16::try_from(item_index).map_err(|_| {
                    RepositoryError::InvalidData("item index exceeds SMALLINT".into())
                })?,
                input,
            )
            .await?;
        }

        transaction.commit().await?;
        let job_id = decode_job_id(job_id)?;
        self.get(job_id)
            .await?
            .ok_or(RepositoryError::StateConflict("read created image job"))
    }

    pub async fn get_downloadable_artifact(
        &self,
        artifact_id: u64,
    ) -> Result<Option<ArtifactRecord>, RepositoryError> {
        let artifact_id = encode_u64(artifact_id, "artifact ID")?;
        let row = sqlx::query_as::<_, ArtifactRow>(
            r#"
            SELECT
                artifact.id,
                artifact.job_id,
                artifact.attempt_id,
                artifact.source_artifact_id,
                artifact.kind,
                artifact.item_index,
                artifact.storage_key,
                artifact.display_name,
                artifact.media_type,
                artifact.byte_size,
                artifact.width,
                artifact.height,
                artifact.created_at
            FROM artifacts AS artifact
            INNER JOIN jobs AS job ON job.id = artifact.job_id
            WHERE artifact.id = $1
              AND artifact.kind = 'output'
              AND job.state = 'succeeded'
            "#,
        )
        .bind(artifact_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(TryInto::try_into).transpose()
    }

    pub async fn update_progress(
        &self,
        claimed: &ClaimedJob,
        completed: u32,
    ) -> Result<(), RepositoryError> {
        let completed = i32::try_from(completed)
            .map_err(|_| RepositoryError::InvalidData("progress exceeds INTEGER".into()))?;
        let job_id = encode_job_id(claimed.job_id())?;
        let mut transaction = self.pool.begin().await?;

        let attempt = sqlx::query(
            r#"
            UPDATE job_attempts
            SET progress_completed = $3
            WHERE id = $1
              AND job_id = $2
              AND state = 'running'
              AND worker_id = $4
              AND claim_token = $5
              AND lease_expires_at > CURRENT_TIMESTAMP
              AND $3 BETWEEN progress_completed AND progress_total
            "#,
        )
        .bind(claimed.attempt_id())
        .bind(job_id)
        .bind(completed)
        .bind(claimed.worker_id().as_uuid())
        .bind(claimed.claim_token())
        .execute(&mut *transaction)
        .await?;
        ensure_claim_row(attempt.rows_affected())?;

        let job = sqlx::query(
            r#"
            UPDATE jobs
            SET progress_completed = $2
            WHERE id = $1
              AND state = 'running'
              AND $2 BETWEEN progress_completed AND progress_total
            "#,
        )
        .bind(job_id)
        .bind(completed)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(job.rows_affected(), "update job progress")?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn complete_image_job(
        &self,
        claimed: &ClaimedJob,
        duration: Duration,
        outputs: Vec<PendingOutputArtifact>,
    ) -> Result<(), RepositoryError> {
        let ClaimedWork::ImageResize { inputs, .. } = claimed.work() else {
            return Err(RepositoryError::InvalidData(
                "demo job cannot publish image outputs".into(),
            ));
        };
        if outputs.len() != inputs.len() {
            return Err(RepositoryError::InvalidData(
                "output count does not match input count".into(),
            ));
        }

        let duration_ms = encode_duration(duration)?;
        let job_id = encode_job_id(claimed.job_id())?;
        let mut transaction = self.pool.begin().await?;
        lock_active_claim(&mut transaction, claimed, job_id).await?;
        for output in outputs {
            insert_output_artifact(&mut transaction, job_id, claimed.attempt_id(), output).await?;
        }

        complete_attempt(&mut transaction, claimed, job_id, duration_ms).await?;
        let noun = if inputs.len() == 1 { "image" } else { "images" };
        let message = format!("{} {noun} resized", inputs.len());
        let job = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'succeeded',
                progress_completed = progress_total,
                result_message = $2,
                result_duration_ms = $3,
                failure_message = NULL,
                finished_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND state = 'running'
              AND job_type = 'image_resize'
            "#,
        )
        .bind(job_id)
        .bind(message)
        .bind(duration_ms)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(job.rows_affected(), "complete image job")?;

        transaction.commit().await?;
        Ok(())
    }
}

pub(crate) async fn load_artifacts(
    pool: &PgPool,
    job_id: i64,
) -> Result<Vec<ArtifactRecord>, RepositoryError> {
    sqlx::query_as::<_, ArtifactRow>(
        r#"
        SELECT
            id,
            job_id,
            attempt_id,
            source_artifact_id,
            kind,
            item_index,
            storage_key,
            display_name,
            media_type,
            byte_size,
            width,
            height,
            created_at
        FROM artifacts
        WHERE job_id = $1
        ORDER BY kind, item_index
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn load_input_artifacts(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
) -> Result<Vec<ArtifactRecord>, RepositoryError> {
    sqlx::query_as::<_, ArtifactRow>(
        r#"
        SELECT
            id,
            job_id,
            attempt_id,
            source_artifact_id,
            kind,
            item_index,
            storage_key,
            display_name,
            media_type,
            byte_size,
            width,
            height,
            created_at
        FROM artifacts
        WHERE job_id = $1
          AND kind = 'input'
        ORDER BY item_index
        "#,
    )
    .bind(job_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

#[derive(Debug, FromRow)]
struct ArtifactRow {
    id: i64,
    job_id: i64,
    attempt_id: Option<i64>,
    source_artifact_id: Option<i64>,
    kind: String,
    item_index: i16,
    storage_key: String,
    display_name: String,
    media_type: String,
    byte_size: i64,
    width: i32,
    height: i32,
    created_at: OffsetDateTime,
}

impl TryFrom<ArtifactRow> for ArtifactRecord {
    type Error = RepositoryError;

    fn try_from(row: ArtifactRow) -> Result<Self, Self::Error> {
        let kind = match row.kind.as_str() {
            "input" => ArtifactKind::Input,
            "output" => ArtifactKind::Output,
            _ => {
                return Err(RepositoryError::InvalidData(
                    "artifact kind is not recognized".into(),
                ));
            }
        };
        if !matches!(row.media_type.as_str(), "image/jpeg" | "image/png") {
            return Err(RepositoryError::InvalidData(
                "artifact media type is not recognized".into(),
            ));
        }

        Ok(Self {
            id: decode_u64(row.id, "artifact ID")?,
            job_id: decode_job_id(row.job_id)?,
            attempt_id: row.attempt_id,
            source_artifact_id: row
                .source_artifact_id
                .map(|value| decode_u64(value, "source artifact ID"))
                .transpose()?,
            kind,
            item_index: u16::try_from(row.item_index)
                .map_err(|_| RepositoryError::InvalidData("invalid item index".into()))?,
            storage_key: row.storage_key,
            display_name: row.display_name,
            media_type: row.media_type,
            byte_size: decode_u64(row.byte_size, "artifact byte size")?,
            width: decode_u32(row.width, "artifact width")?,
            height: decode_u32(row.height, "artifact height")?,
            created_at: row.created_at,
        })
    }
}

async fn insert_input_artifact(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    item_index: i16,
    input: NewInputArtifact,
) -> Result<(), RepositoryError> {
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
        VALUES ($1, 'input', $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(job_id)
    .bind(item_index)
    .bind(input.storage_key)
    .bind(input.display_name)
    .bind(input.media_type)
    .bind(encode_u64(input.byte_size, "input byte size")?)
    .bind(
        i32::try_from(input.width)
            .map_err(|_| RepositoryError::InvalidData("input width exceeds INTEGER".into()))?,
    )
    .bind(
        i32::try_from(input.height)
            .map_err(|_| RepositoryError::InvalidData("input height exceeds INTEGER".into()))?,
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_output_artifact(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: i64,
    attempt_id: i64,
    output: PendingOutputArtifact,
) -> Result<(), RepositoryError> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO artifacts (
            job_id,
            attempt_id,
            source_artifact_id,
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
            $1,
            $2,
            source.id,
            'output',
            $3,
            $4,
            $5,
            $6,
            $7,
            $8,
            $9
        FROM artifacts AS source
        WHERE source.id = $10
          AND source.job_id = $1
          AND source.kind = 'input'
          AND source.item_index = $3
        "#,
    )
    .bind(job_id)
    .bind(attempt_id)
    .bind(
        i16::try_from(output.item_index).map_err(|_| {
            RepositoryError::InvalidData("output item index exceeds SMALLINT".into())
        })?,
    )
    .bind(output.storage_key)
    .bind(output.display_name)
    .bind(output.media_type)
    .bind(encode_u64(output.byte_size, "output byte size")?)
    .bind(
        i32::try_from(output.width)
            .map_err(|_| RepositoryError::InvalidData("output width exceeds INTEGER".into()))?,
    )
    .bind(
        i32::try_from(output.height)
            .map_err(|_| RepositoryError::InvalidData("output height exceeds INTEGER".into()))?,
    )
    .bind(encode_u64(output.source_artifact_id, "source artifact ID")?)
    .execute(&mut **transaction)
    .await?;
    ensure_one_row(inserted.rows_affected(), "publish output artifact")
}

async fn complete_attempt(
    transaction: &mut Transaction<'_, Postgres>,
    claimed: &ClaimedJob,
    job_id: i64,
    duration_ms: i64,
) -> Result<(), RepositoryError> {
    let attempt = sqlx::query(
        r#"
        UPDATE job_attempts
        SET state = 'succeeded',
            progress_completed = progress_total,
            finished_at = CURRENT_TIMESTAMP,
            duration_ms = $3
        WHERE id = $1
          AND job_id = $2
          AND state = 'running'
          AND worker_id = $4
          AND claim_token = $5
          AND lease_expires_at > CURRENT_TIMESTAMP
        "#,
    )
    .bind(claimed.attempt_id())
    .bind(job_id)
    .bind(duration_ms)
    .bind(claimed.worker_id().as_uuid())
    .bind(claimed.claim_token())
    .execute(&mut **transaction)
    .await?;
    ensure_claim_row(attempt.rows_affected())
}

async fn lock_active_claim(
    transaction: &mut Transaction<'_, Postgres>,
    claimed: &ClaimedJob,
    job_id: i64,
) -> Result<(), RepositoryError> {
    let active = sqlx::query_scalar::<_, i32>(
        r#"
        SELECT 1
        FROM job_attempts AS attempt
        INNER JOIN jobs AS job ON job.id = attempt.job_id
        WHERE attempt.id = $1
          AND attempt.job_id = $2
          AND attempt.worker_id = $3
          AND attempt.claim_token = $4
          AND attempt.state = 'running'
          AND attempt.lease_expires_at > CURRENT_TIMESTAMP
          AND job.state = 'running'
        FOR UPDATE OF attempt, job
        "#,
    )
    .bind(claimed.attempt_id())
    .bind(job_id)
    .bind(claimed.worker_id().as_uuid())
    .bind(claimed.claim_token())
    .fetch_optional(&mut **transaction)
    .await?;
    if active.is_some() {
        Ok(())
    } else {
        Err(RepositoryError::ClaimLost)
    }
}

fn encode_duration(value: Duration) -> Result<i64, RepositoryError> {
    i64::try_from(value.as_millis())
        .map_err(|_| RepositoryError::InvalidData("worker duration exceeds BIGINT".into()))
}

fn encode_u64(value: u64, field: &str) -> Result<i64, RepositoryError> {
    i64::try_from(value)
        .map_err(|_| RepositoryError::InvalidData(format!("{field} exceeds BIGINT")))
}
