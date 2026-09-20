mod image_processing;
mod image_repository;
mod lifecycle_repository;
mod postgres;
mod schedule_repository;
mod storage;
mod worker_repository;

pub use image_processing::{
    DEFAULT_JPEG_QUALITY, DEFAULT_OUTPUT_WIDTH, ImageError, ImageMetadata, ImageService,
    MAX_FILE_BYTES, MAX_FILES_PER_JOB, MAX_IMAGE_DIMENSION, MAX_IMAGE_PIXELS, MAX_OUTPUT_WIDTH,
    MAX_TOTAL_FILE_BYTES, ProcessedImage,
};
pub use image_repository::{
    ArtifactKind, ArtifactRecord, JobSettings, NewImageJob, NewInputArtifact, PendingOutputArtifact,
};
pub use lifecycle_repository::{
    AttemptRecord, AttemptStatus, DEFAULT_MAX_ATTEMPTS, FailureDisposition, FailureKind,
    MAX_ATTEMPTS,
};
pub use postgres::{
    ClaimedJob, ClaimedWork, DEMO_DELAY_MS, JobRecord, PgJobRepository, RepositoryError,
};
pub use schedule_repository::{
    MAX_SCHEDULE_INTERVAL_SECONDS, MIN_SCHEDULE_INTERVAL_SECONDS, NewSchedule, ScheduleInputRecord,
    ScheduleOccurrenceOutcome, ScheduleOccurrenceRecord, ScheduleRecord, ScheduleTick,
    UpdateSchedule,
};
pub use storage::{LocalStorage, StorageError, UploadBatch};
pub use worker_repository::{
    MAX_LEASE_DURATION, MAX_WORKER_CONCURRENCY, MIN_LEASE_DURATION, MIN_WORKER_CONCURRENCY,
    ReclaimDisposition, ReclaimedAttempt, WorkerId, WorkerRecord, WorkerRegistration, WorkerStatus,
};
