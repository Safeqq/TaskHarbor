use sqlx::{FromRow, PgPool, Postgres, Transaction};
use taskharbor_core::{JobId, JobName, JobPriority, JobStatus, ScheduleId};
use time::{Duration, OffsetDateTime};

use crate::auth_repository::UserId;
use crate::image_processing::{MAX_FILES_PER_JOB, MAX_OUTPUT_WIDTH};
use crate::image_repository::NewInputArtifact;
use crate::postgres::{
    PgJobRepository, RepositoryError, decode_job_id, decode_schedule_id, decode_u32, decode_u64,
    encode_schedule_id, ensure_one_row,
};

pub const MIN_SCHEDULE_INTERVAL_SECONDS: u32 = 60;
pub const MAX_SCHEDULE_INTERVAL_SECONDS: u32 = 31_536_000;
const OVERLAP_REASON: &str = "previous occurrence is still active";

#[derive(Debug)]
pub struct NewSchedule {
    pub owner_user_id: UserId,
    pub name: JobName,
    pub interval_seconds: u32,
    pub anchor_at: OffsetDateTime,
    pub priority: JobPriority,
    pub max_width: u32,
    pub jpeg_quality: u8,
    pub inputs: Vec<NewInputArtifact>,
}

impl NewSchedule {
    fn validate(&self) -> Result<(), RepositoryError> {
        validate_schedule_fields(
            self.interval_seconds,
            self.max_width,
            self.jpeg_quality,
            self.inputs.len(),
        )
    }
}

#[derive(Debug)]
pub struct UpdateSchedule {
    pub name: JobName,
    pub enabled: bool,
    pub interval_seconds: u32,
    pub anchor_at: OffsetDateTime,
    pub priority: JobPriority,
    pub max_width: u32,
    pub jpeg_quality: u8,
}

impl UpdateSchedule {
    fn validate(&self) -> Result<(), RepositoryError> {
        validate_schedule_fields(self.interval_seconds, self.max_width, self.jpeg_quality, 1)
    }
}

fn validate_schedule_fields(
    interval_seconds: u32,
    max_width: u32,
    jpeg_quality: u8,
    input_count: usize,
) -> Result<(), RepositoryError> {
    if !(MIN_SCHEDULE_INTERVAL_SECONDS..=MAX_SCHEDULE_INTERVAL_SECONDS).contains(&interval_seconds)
    {
        return Err(RepositoryError::InvalidData(format!(
            "schedule interval must be between {MIN_SCHEDULE_INTERVAL_SECONDS} and {MAX_SCHEDULE_INTERVAL_SECONDS} seconds"
        )));
    }
    if max_width == 0 || max_width > MAX_OUTPUT_WIDTH || !(1..=100).contains(&jpeg_quality) {
        return Err(RepositoryError::InvalidData(
            "schedule image settings are outside the supported range".into(),
        ));
    }
    if input_count == 0 || input_count > MAX_FILES_PER_JOB {
        return Err(RepositoryError::InvalidData(format!(
            "schedule must contain between 1 and {MAX_FILES_PER_JOB} inputs"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ScheduleRecord {
    id: ScheduleId,
    name: JobName,
    enabled: bool,
    interval_seconds: u32,
    anchor_at: OffsetDateTime,
    next_run_at: OffsetDateTime,
    priority: JobPriority,
    max_width: u32,
    jpeg_quality: u8,
    inputs: Vec<ScheduleInputRecord>,
    occurrences: Vec<ScheduleOccurrenceRecord>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl ScheduleRecord {
    pub const fn id(&self) -> ScheduleId {
        self.id
    }

    pub fn name(&self) -> &JobName {
        &self.name
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn interval_seconds(&self) -> u32 {
        self.interval_seconds
    }

    pub const fn anchor_at(&self) -> OffsetDateTime {
        self.anchor_at
    }

    pub const fn next_run_at(&self) -> OffsetDateTime {
        self.next_run_at
    }

    pub const fn priority(&self) -> JobPriority {
        self.priority
    }

    pub const fn max_width(&self) -> u32 {
        self.max_width
    }

    pub const fn jpeg_quality(&self) -> u8 {
        self.jpeg_quality
    }

    pub fn inputs(&self) -> impl Iterator<Item = &ScheduleInputRecord> {
        self.inputs.iter()
    }

    pub fn occurrences(&self) -> impl Iterator<Item = &ScheduleOccurrenceRecord> {
        self.occurrences.iter()
    }

    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    pub const fn updated_at(&self) -> OffsetDateTime {
        self.updated_at
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleInputRecord {
    id: u64,
    item_index: u16,
    storage_key: String,
    display_name: String,
    media_type: String,
    byte_size: u64,
    width: u32,
    height: u32,
}

impl ScheduleInputRecord {
    pub const fn id(&self) -> u64 {
        self.id
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleOccurrenceOutcome {
    Created,
    SkippedOverlap,
}

impl ScheduleOccurrenceOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::SkippedOverlap => "skipped_overlap",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleOccurrenceRecord {
    id: u64,
    scheduled_for: OffsetDateTime,
    outcome: ScheduleOccurrenceOutcome,
    job_id: Option<JobId>,
    job_status: Option<JobStatus>,
    reason: Option<String>,
    coalesced_slots: u64,
    created_at: OffsetDateTime,
}

impl ScheduleOccurrenceRecord {
    pub const fn id(&self) -> u64 {
        self.id
    }

    pub const fn scheduled_for(&self) -> OffsetDateTime {
        self.scheduled_for
    }

    pub const fn outcome(&self) -> ScheduleOccurrenceOutcome {
        self.outcome
    }

    pub const fn job_id(&self) -> Option<JobId> {
        self.job_id
    }

    pub const fn job_status(&self) -> Option<JobStatus> {
        self.job_status
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub const fn coalesced_slots(&self) -> u64 {
        self.coalesced_slots
    }

    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScheduleTick {
    schedule_id: ScheduleId,
    scheduled_for: OffsetDateTime,
    outcome: ScheduleOccurrenceOutcome,
    job_id: Option<JobId>,
    coalesced_slots: u64,
}

impl ScheduleTick {
    pub const fn schedule_id(self) -> ScheduleId {
        self.schedule_id
    }

    pub const fn scheduled_for(self) -> OffsetDateTime {
        self.scheduled_for
    }

    pub const fn outcome(self) -> ScheduleOccurrenceOutcome {
        self.outcome
    }

    pub const fn job_id(self) -> Option<JobId> {
        self.job_id
    }

    pub const fn coalesced_slots(self) -> u64 {
        self.coalesced_slots
    }
}

impl PgJobRepository {
    pub async fn create_schedule(
        &self,
        request: NewSchedule,
    ) -> Result<ScheduleRecord, RepositoryError> {
        request.validate()?;
        let mut transaction = self.pool.begin().await?;
        let schedule_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO schedules (
                name,
                interval_seconds,
                anchor_at,
                next_run_at,
                priority,
                max_width,
                jpeg_quality
                , owner_user_id
            )
            VALUES ($1, $2, $3, $3, $4, $5, $6, $7)
            RETURNING id
            "#,
        )
        .bind(request.name.as_str())
        .bind(encode_u32(request.interval_seconds, "schedule interval")?)
        .bind(request.anchor_at)
        .bind(request.priority.as_str())
        .bind(encode_u32(request.max_width, "schedule max width")?)
        .bind(i16::from(request.jpeg_quality))
        .bind(request.owner_user_id.get())
        .fetch_one(&mut *transaction)
        .await?;

        for (item_index, input) in request.inputs.into_iter().enumerate() {
            insert_schedule_input(
                &mut transaction,
                schedule_id,
                i16::try_from(item_index).map_err(|_| {
                    RepositoryError::InvalidData("schedule item index exceeds SMALLINT".into())
                })?,
                input,
            )
            .await?;
        }

        transaction.commit().await?;
        self.get_schedule(decode_schedule_id(schedule_id)?)
            .await?
            .ok_or(RepositoryError::StateConflict("read created schedule"))
    }

    pub async fn list_schedules(&self) -> Result<Vec<ScheduleRecord>, RepositoryError> {
        self.list_schedules_with_owner(None).await
    }

    pub async fn list_schedules_for_owner(
        &self,
        owner_user_id: UserId,
    ) -> Result<Vec<ScheduleRecord>, RepositoryError> {
        self.list_schedules_with_owner(Some(owner_user_id)).await
    }

    async fn list_schedules_with_owner(
        &self,
        owner_user_id: Option<UserId>,
    ) -> Result<Vec<ScheduleRecord>, RepositoryError> {
        let rows = sqlx::query_as::<_, ScheduleRow>(
            r#"
            SELECT
                id,
                name,
                enabled,
                interval_seconds,
                anchor_at,
                next_run_at,
                priority,
                max_width,
                jpeg_quality,
                created_at,
                updated_at
            FROM schedules
            WHERE $1::BIGINT IS NULL OR owner_user_id = $1
            ORDER BY id
            "#,
        )
        .bind(owner_user_id.map(UserId::get))
        .fetch_all(&self.pool)
        .await?;

        let mut schedules = Vec::with_capacity(rows.len());
        for row in rows {
            let inputs = load_schedule_inputs(&self.pool, row.id).await?;
            let occurrences = load_schedule_occurrences(&self.pool, row.id).await?;
            schedules.push(row.try_into_record(inputs, occurrences)?);
        }
        Ok(schedules)
    }

    pub async fn get_schedule(
        &self,
        id: ScheduleId,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        self.get_schedule_with_owner(id, None).await
    }

    pub async fn get_schedule_for_owner(
        &self,
        id: ScheduleId,
        owner_user_id: UserId,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        self.get_schedule_with_owner(id, Some(owner_user_id)).await
    }

    async fn get_schedule_with_owner(
        &self,
        id: ScheduleId,
        owner_user_id: Option<UserId>,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        let raw_id = encode_schedule_id(id)?;
        let row = sqlx::query_as::<_, ScheduleRow>(
            r#"
            SELECT
                id,
                name,
                enabled,
                interval_seconds,
                anchor_at,
                next_run_at,
                priority,
                max_width,
                jpeg_quality,
                created_at,
                updated_at
            FROM schedules
            WHERE id = $1
              AND ($2::BIGINT IS NULL OR owner_user_id = $2)
            "#,
        )
        .bind(raw_id)
        .bind(owner_user_id.map(UserId::get))
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let inputs = load_schedule_inputs(&self.pool, raw_id).await?;
        let occurrences = load_schedule_occurrences(&self.pool, raw_id).await?;
        Ok(Some(row.try_into_record(inputs, occurrences)?))
    }

    pub async fn update_schedule(
        &self,
        id: ScheduleId,
        request: UpdateSchedule,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        let as_of = sqlx::query_scalar::<_, OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&self.pool)
            .await?;
        self.update_schedule_at_with_owner(id, request, as_of, None)
            .await
    }

    pub async fn update_schedule_for_owner(
        &self,
        id: ScheduleId,
        request: UpdateSchedule,
        owner_user_id: UserId,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        let as_of = sqlx::query_scalar::<_, OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&self.pool)
            .await?;
        self.update_schedule_at_with_owner(id, request, as_of, Some(owner_user_id))
            .await
    }

    pub async fn update_schedule_at(
        &self,
        id: ScheduleId,
        request: UpdateSchedule,
        as_of: OffsetDateTime,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        self.update_schedule_at_with_owner(id, request, as_of, None)
            .await
    }

    async fn update_schedule_at_with_owner(
        &self,
        id: ScheduleId,
        request: UpdateSchedule,
        as_of: OffsetDateTime,
        owner_user_id: Option<UserId>,
    ) -> Result<Option<ScheduleRecord>, RepositoryError> {
        request.validate()?;
        let raw_id = encode_schedule_id(id)?;
        let next_run_at = next_slot_after(request.anchor_at, request.interval_seconds, as_of)?;
        let updated = sqlx::query(
            r#"
            UPDATE schedules
            SET name = $2,
                enabled = $3,
                interval_seconds = $4,
                anchor_at = $5,
                next_run_at = $6,
                priority = $7,
                max_width = $8,
                jpeg_quality = $9,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND ($10::BIGINT IS NULL OR owner_user_id = $10)
            "#,
        )
        .bind(raw_id)
        .bind(request.name.as_str())
        .bind(request.enabled)
        .bind(encode_u32(request.interval_seconds, "schedule interval")?)
        .bind(request.anchor_at)
        .bind(next_run_at)
        .bind(request.priority.as_str())
        .bind(encode_u32(request.max_width, "schedule max width")?)
        .bind(i16::from(request.jpeg_quality))
        .bind(owner_user_id.map(UserId::get))
        .execute(&self.pool)
        .await?;

        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        match owner_user_id {
            Some(owner_user_id) => self.get_schedule_for_owner(id, owner_user_id).await,
            None => self.get_schedule(id).await,
        }
    }

    pub async fn materialize_next_schedule(&self) -> Result<Option<ScheduleTick>, RepositoryError> {
        let as_of = sqlx::query_scalar::<_, OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&self.pool)
            .await?;
        self.materialize_next_schedule_at(as_of).await
    }

    pub async fn materialize_next_schedule_at(
        &self,
        as_of: OffsetDateTime,
    ) -> Result<Option<ScheduleTick>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let candidate = sqlx::query_as::<_, ScheduleCandidateRow>(
            r#"
            SELECT
                schedule.id,
                schedule.name,
                schedule.interval_seconds,
                schedule.next_run_at,
                schedule.priority,
                schedule.max_width,
                schedule.jpeg_quality,
                schedule.owner_user_id,
                (
                    SELECT COUNT(*)
                    FROM schedule_inputs AS input
                    WHERE input.schedule_id = schedule.id
                ) AS input_count
            FROM schedules AS schedule
            WHERE schedule.enabled
              AND schedule.next_run_at <= $1
            ORDER BY schedule.next_run_at, schedule.id
            FOR UPDATE SKIP LOCKED
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
        let interval_seconds = decode_u32(candidate.interval_seconds, "schedule interval")?;
        let (scheduled_for, next_run_at, coalesced_slots) =
            latest_due_slot(candidate.next_run_at, interval_seconds, as_of)?;
        let input_count = decode_u32_i64(candidate.input_count, "schedule input count")?;
        if input_count == 0 || input_count > MAX_FILES_PER_JOB as u32 {
            return Err(RepositoryError::InvalidData(
                "schedule input count violates its invariant".into(),
            ));
        }

        let has_overlap = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM jobs
                WHERE schedule_id = $1
                  AND state IN ('queued', 'running', 'retry_waiting', 'cancel_requested')
            )
            "#,
        )
        .bind(candidate.id)
        .fetch_one(&mut *transaction)
        .await?;

        let (outcome, job_id, reason) = if has_overlap {
            (
                ScheduleOccurrenceOutcome::SkippedOverlap,
                None,
                Some(OVERLAP_REASON),
            )
        } else {
            let job_id = sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO jobs (
                    name,
                    job_type,
                    available_at,
                    progress_total,
                    max_width,
                    jpeg_quality,
                    priority,
                    schedule_id,
                    scheduled_for
                    , owner_user_id
                )
                VALUES ($1, 'image_resize', $2, $3, $4, $5, $6, $7, $2, $8)
                RETURNING id
                "#,
            )
            .bind(&candidate.name)
            .bind(scheduled_for)
            .bind(encode_u32(input_count, "schedule input count")?)
            .bind(candidate.max_width)
            .bind(candidate.jpeg_quality)
            .bind(&candidate.priority)
            .bind(candidate.id)
            .bind(candidate.owner_user_id)
            .fetch_one(&mut *transaction)
            .await?;

            let copied = sqlx::query(
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
                    height,
                    checksum_sha256
                )
                SELECT
                    $2,
                    'input',
                    item_index,
                    storage_key,
                    display_name,
                    media_type,
                    byte_size,
                    width,
                    height,
                    checksum_sha256
                FROM schedule_inputs
                WHERE schedule_id = $1
                ORDER BY item_index
                "#,
            )
            .bind(candidate.id)
            .bind(job_id)
            .execute(&mut *transaction)
            .await?;
            if copied.rows_affected() != u64::from(input_count) {
                return Err(RepositoryError::StateConflict(
                    "copy schedule inputs into occurrence",
                ));
            }

            (
                ScheduleOccurrenceOutcome::Created,
                Some(decode_job_id(job_id)?),
                None,
            )
        };

        sqlx::query(
            r#"
            INSERT INTO schedule_occurrences (
                schedule_id,
                scheduled_for,
                outcome,
                job_id,
                reason,
                coalesced_slots
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(candidate.id)
        .bind(scheduled_for)
        .bind(outcome.as_str())
        .bind(
            job_id
                .map(|id| i64::try_from(id.get()))
                .transpose()
                .map_err(|_| RepositoryError::InvalidData("job ID exceeds BIGINT".into()))?,
        )
        .bind(reason)
        .bind(encode_u64(coalesced_slots, "coalesced slots")?)
        .execute(&mut *transaction)
        .await?;

        let advanced = sqlx::query(
            r#"
            UPDATE schedules
            SET next_run_at = $2,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(candidate.id)
        .bind(next_run_at)
        .execute(&mut *transaction)
        .await?;
        ensure_one_row(advanced.rows_affected(), "advance schedule cursor")?;

        transaction.commit().await?;
        Ok(Some(ScheduleTick {
            schedule_id: decode_schedule_id(candidate.id)?,
            scheduled_for,
            outcome,
            job_id,
            coalesced_slots,
        }))
    }
}

async fn insert_schedule_input(
    transaction: &mut Transaction<'_, Postgres>,
    schedule_id: i64,
    item_index: i16,
    input: NewInputArtifact,
) -> Result<(), RepositoryError> {
    sqlx::query(
        r#"
        INSERT INTO schedule_inputs (
            schedule_id,
            item_index,
            storage_key,
            display_name,
            media_type,
            byte_size,
            width,
            height,
            checksum_sha256
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(schedule_id)
    .bind(item_index)
    .bind(input.storage_key)
    .bind(input.display_name)
    .bind(input.media_type)
    .bind(encode_u64(input.byte_size, "schedule input byte size")?)
    .bind(encode_u32(input.width, "schedule input width")?)
    .bind(encode_u32(input.height, "schedule input height")?)
    .bind(input.checksum_sha256)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn load_schedule_inputs(
    pool: &PgPool,
    schedule_id: i64,
) -> Result<Vec<ScheduleInputRecord>, RepositoryError> {
    sqlx::query_as::<_, ScheduleInputRow>(
        r#"
        SELECT id, item_index, storage_key, display_name, media_type, byte_size, width, height
        FROM schedule_inputs
        WHERE schedule_id = $1
        ORDER BY item_index
        "#,
    )
    .bind(schedule_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

async fn load_schedule_occurrences(
    pool: &PgPool,
    schedule_id: i64,
) -> Result<Vec<ScheduleOccurrenceRecord>, RepositoryError> {
    sqlx::query_as::<_, ScheduleOccurrenceRow>(
        r#"
        SELECT
            occurrence.id,
            occurrence.scheduled_for,
            occurrence.outcome,
            occurrence.job_id,
            job.state AS job_status,
            occurrence.reason,
            occurrence.coalesced_slots,
            occurrence.created_at
        FROM schedule_occurrences AS occurrence
        LEFT JOIN jobs AS job ON job.id = occurrence.job_id
        WHERE occurrence.schedule_id = $1
        ORDER BY occurrence.scheduled_for DESC
        "#,
    )
    .bind(schedule_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

fn latest_due_slot(
    next_run_at: OffsetDateTime,
    interval_seconds: u32,
    as_of: OffsetDateTime,
) -> Result<(OffsetDateTime, OffsetDateTime, u64), RepositoryError> {
    if next_run_at > as_of {
        return Err(RepositoryError::InvalidData(
            "schedule is not due at the supplied clock time".into(),
        ));
    }
    let interval = i64::from(interval_seconds);
    let elapsed = (as_of - next_run_at).whole_seconds();
    let coalesced_slots = u64::try_from(elapsed / interval).map_err(|_| {
        RepositoryError::InvalidData("schedule clock moved before its cursor".into())
    })?;
    let offset_seconds = coalesced_slots
        .checked_mul(u64::from(interval_seconds))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| RepositoryError::InvalidData("schedule offset is too large".into()))?;
    let scheduled_for = next_run_at
        .checked_add(Duration::seconds(offset_seconds))
        .ok_or_else(|| RepositoryError::InvalidData("scheduled time overflowed".into()))?;
    let next = scheduled_for
        .checked_add(Duration::seconds(interval))
        .ok_or_else(|| RepositoryError::InvalidData("next schedule time overflowed".into()))?;
    Ok((scheduled_for, next, coalesced_slots))
}

fn next_slot_after(
    anchor_at: OffsetDateTime,
    interval_seconds: u32,
    as_of: OffsetDateTime,
) -> Result<OffsetDateTime, RepositoryError> {
    if anchor_at > as_of {
        return Ok(anchor_at);
    }
    let elapsed_nanos = (as_of - anchor_at).whole_nanoseconds();
    let interval_nanos = i128::from(interval_seconds) * 1_000_000_000;
    let steps = elapsed_nanos
        .checked_div(interval_nanos)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| RepositoryError::InvalidData("schedule interval overflowed".into()))?;
    let seconds = steps
        .checked_mul(i128::from(interval_seconds))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| RepositoryError::InvalidData("schedule cursor is too large".into()))?;
    anchor_at
        .checked_add(Duration::seconds(seconds))
        .ok_or_else(|| RepositoryError::InvalidData("schedule cursor overflowed".into()))
}

fn encode_u32(value: u32, field: &str) -> Result<i32, RepositoryError> {
    i32::try_from(value)
        .map_err(|_| RepositoryError::InvalidData(format!("{field} exceeds INTEGER")))
}

fn encode_u64(value: u64, field: &str) -> Result<i64, RepositoryError> {
    i64::try_from(value)
        .map_err(|_| RepositoryError::InvalidData(format!("{field} exceeds BIGINT")))
}

fn decode_u32_i64(value: i64, field: &str) -> Result<u32, RepositoryError> {
    u32::try_from(value).map_err(|_| RepositoryError::InvalidData(format!("invalid {field}")))
}

#[derive(Debug, FromRow)]
struct ScheduleRow {
    id: i64,
    name: String,
    enabled: bool,
    interval_seconds: i32,
    anchor_at: OffsetDateTime,
    next_run_at: OffsetDateTime,
    priority: String,
    max_width: i32,
    jpeg_quality: i16,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl ScheduleRow {
    fn try_into_record(
        self,
        inputs: Vec<ScheduleInputRecord>,
        occurrences: Vec<ScheduleOccurrenceRecord>,
    ) -> Result<ScheduleRecord, RepositoryError> {
        let priority = self
            .priority
            .parse::<JobPriority>()
            .map_err(|error| RepositoryError::InvalidData(error.to_string()))?;
        let jpeg_quality = u8::try_from(self.jpeg_quality)
            .map_err(|_| RepositoryError::InvalidData("invalid schedule JPEG quality".into()))?;
        validate_schedule_fields(
            decode_u32(self.interval_seconds, "schedule interval")?,
            decode_u32(self.max_width, "schedule max width")?,
            jpeg_quality,
            inputs.len(),
        )?;

        Ok(ScheduleRecord {
            id: decode_schedule_id(self.id)?,
            name: JobName::new(self.name)
                .map_err(|error| RepositoryError::InvalidData(error.to_string()))?,
            enabled: self.enabled,
            interval_seconds: decode_u32(self.interval_seconds, "schedule interval")?,
            anchor_at: self.anchor_at,
            next_run_at: self.next_run_at,
            priority,
            max_width: decode_u32(self.max_width, "schedule max width")?,
            jpeg_quality,
            inputs,
            occurrences,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct ScheduleCandidateRow {
    id: i64,
    name: String,
    interval_seconds: i32,
    next_run_at: OffsetDateTime,
    priority: String,
    max_width: i32,
    jpeg_quality: i16,
    owner_user_id: i64,
    input_count: i64,
}

#[derive(Debug, FromRow)]
struct ScheduleInputRow {
    id: i64,
    item_index: i16,
    storage_key: String,
    display_name: String,
    media_type: String,
    byte_size: i64,
    width: i32,
    height: i32,
}

impl TryFrom<ScheduleInputRow> for ScheduleInputRecord {
    type Error = RepositoryError;

    fn try_from(row: ScheduleInputRow) -> Result<Self, Self::Error> {
        if !matches!(row.media_type.as_str(), "image/jpeg" | "image/png") {
            return Err(RepositoryError::InvalidData(
                "schedule input media type is not recognized".into(),
            ));
        }
        Ok(Self {
            id: decode_u64(row.id, "schedule input ID")?,
            item_index: u16::try_from(row.item_index)
                .map_err(|_| RepositoryError::InvalidData("invalid schedule item index".into()))?,
            storage_key: row.storage_key,
            display_name: row.display_name,
            media_type: row.media_type,
            byte_size: decode_u64(row.byte_size, "schedule input byte size")?,
            width: decode_u32(row.width, "schedule input width")?,
            height: decode_u32(row.height, "schedule input height")?,
        })
    }
}

#[derive(Debug, FromRow)]
struct ScheduleOccurrenceRow {
    id: i64,
    scheduled_for: OffsetDateTime,
    outcome: String,
    job_id: Option<i64>,
    job_status: Option<String>,
    reason: Option<String>,
    coalesced_slots: i64,
    created_at: OffsetDateTime,
}

impl TryFrom<ScheduleOccurrenceRow> for ScheduleOccurrenceRecord {
    type Error = RepositoryError;

    fn try_from(row: ScheduleOccurrenceRow) -> Result<Self, Self::Error> {
        let outcome = match row.outcome.as_str() {
            "created" => ScheduleOccurrenceOutcome::Created,
            "skipped_overlap" => ScheduleOccurrenceOutcome::SkippedOverlap,
            _ => {
                return Err(RepositoryError::InvalidData(
                    "schedule occurrence outcome is not recognized".into(),
                ));
            }
        };
        let job_id = row.job_id.map(decode_job_id).transpose()?;
        let job_status = row
            .job_status
            .map(|value| {
                value
                    .parse::<JobStatus>()
                    .map_err(|error| RepositoryError::InvalidData(error.to_string()))
            })
            .transpose()?;
        let result_is_valid = match outcome {
            ScheduleOccurrenceOutcome::Created => {
                job_id.is_some() && job_status.is_some() && row.reason.is_none()
            }
            ScheduleOccurrenceOutcome::SkippedOverlap => {
                job_id.is_none() && job_status.is_none() && row.reason.is_some()
            }
        };
        if !result_is_valid {
            return Err(RepositoryError::InvalidData(
                "schedule occurrence result violates its invariant".into(),
            ));
        }

        Ok(Self {
            id: decode_u64(row.id, "schedule occurrence ID")?,
            scheduled_for: row.scheduled_for,
            outcome,
            job_id,
            job_status,
            reason: row.reason,
            coalesced_slots: decode_u64(row.coalesced_slots, "coalesced slots")?,
            created_at: row.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use time::{Duration, OffsetDateTime};

    use super::{latest_due_slot, next_slot_after};

    #[test]
    fn recurrence_math_stays_anchored_and_coalesces_old_slots() {
        let anchor = OffsetDateTime::from_unix_timestamp(1_800_000_000)
            .expect("test timestamp should be valid");
        let as_of = anchor + Duration::seconds(190);

        let (scheduled_for, next_run_at, coalesced) =
            latest_due_slot(anchor, 60, as_of).expect("due slot should be calculated");

        assert_eq!(scheduled_for, anchor + Duration::seconds(180));
        assert_eq!(next_run_at, anchor + Duration::seconds(240));
        assert_eq!(coalesced, 3);
        assert_eq!(
            next_slot_after(anchor, 60, anchor + Duration::seconds(61))
                .expect("future slot should be calculated"),
            anchor + Duration::seconds(120)
        );
        assert_eq!(
            next_slot_after(anchor, 60, anchor).expect("edit should target a future slot"),
            anchor + Duration::seconds(60)
        );
    }
}
