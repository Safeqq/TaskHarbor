use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

pub const MAX_JOB_NAME_LENGTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

impl JobId {
    pub const fn new(value: u64) -> Result<Self, JobIdError> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(JobIdError);
        }

        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Display for JobId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for JobId {
    type Err = JobIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.parse::<u64>().map_err(|_| JobIdError)?;
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobIdError;

impl Display for JobIdError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "job ID must be a positive signed 64-bit integer")
    }
}

impl Error for JobIdError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobName(String);

impl JobName {
    pub fn new(value: impl Into<String>) -> Result<Self, JobNameError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(JobNameError::Empty);
        }

        let actual = value.chars().count();
        if actual > MAX_JOB_NAME_LENGTH {
            return Err(JobNameError::TooLong {
                max: MAX_JOB_NAME_LENGTH,
                actual,
            });
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobNameError {
    Empty,
    TooLong { max: usize, actual: usize },
}

impl Display for JobNameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(
                formatter,
                "job name must contain at least one non-whitespace character"
            ),
            Self::TooLong { max, actual } => write!(
                formatter,
                "job name must contain at most {max} characters, but received {actual}"
            ),
        }
    }
}

impl Error for JobNameError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobType {
    DemoDelay,
    ImageResize,
}

impl JobType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DemoDelay => "demo_delay",
            Self::ImageResize => "image_resize",
        }
    }
}

impl FromStr for JobType {
    type Err = JobTypeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "demo_delay" => Ok(Self::DemoDelay),
            "image_resize" => Ok(Self::ImageResize),
            _ => Err(JobTypeParseError),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobTypeParseError;

impl Display for JobTypeParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "job type is not recognized")
    }
}

impl Error for JobTypeParseError {}

impl JobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

impl FromStr for JobStatus {
    type Err = JobStatusParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(JobStatusParseError),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobStatusParseError;

impl Display for JobStatusParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "job status is not recognized")
    }
}

impl Error for JobStatusParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    name: JobName,
    job_type: JobType,
    status: JobStatus,
}

impl Job {
    pub const fn new_queued(id: JobId, name: JobName, job_type: JobType) -> Self {
        Self {
            id,
            name,
            job_type,
            status: JobStatus::Queued,
        }
    }

    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn status(&self) -> JobStatus {
        self.status
    }

    pub const fn job_type(&self) -> JobType {
        self.job_type
    }

    pub const fn restore(id: JobId, name: JobName, job_type: JobType, status: JobStatus) -> Self {
        Self {
            id,
            name,
            job_type,
            status,
        }
    }

    pub fn name(&self) -> &JobName {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Job, JobId, JobIdError, JobName, JobNameError, JobStatus, JobStatusParseError, JobType,
        JobTypeParseError, MAX_JOB_NAME_LENGTH,
    };

    #[test]
    fn accepts_a_name_at_the_maximum_length() {
        let value = "a".repeat(MAX_JOB_NAME_LENGTH);

        let name = JobName::new(value.clone()).expect("boundary-length name should be valid");

        assert_eq!(name.as_str(), value);
    }

    #[test]
    fn rejects_a_blank_name() {
        let result = JobName::new("   \t");

        assert_eq!(result, Err(JobNameError::Empty));
    }

    #[test]
    fn rejects_a_name_above_the_maximum_length() {
        let actual = MAX_JOB_NAME_LENGTH + 1;

        let result = JobName::new("a".repeat(actual));

        assert_eq!(
            result,
            Err(JobNameError::TooLong {
                max: MAX_JOB_NAME_LENGTH,
                actual,
            })
        );
    }

    #[test]
    fn rejects_zero_as_a_job_id() {
        assert_eq!(JobId::new(0), Err(JobIdError));
    }

    #[test]
    fn rejects_a_job_id_above_the_database_range() {
        assert_eq!(JobId::new(u64::MAX), Err(JobIdError));
    }

    #[test]
    fn creates_a_job_in_the_queued_state() {
        let id = JobId::new(1).expect("test ID should be valid");
        let name = JobName::new("resize profile images").expect("test name should be valid");

        let job = Job::new_queued(id, name, JobType::DemoDelay);

        assert_eq!(job.id(), id);
        assert_eq!(job.job_type(), JobType::DemoDelay);
        assert_eq!(job.status(), JobStatus::Queued);
    }

    #[test]
    fn parses_persisted_job_statuses() {
        assert_eq!("queued".parse(), Ok(JobStatus::Queued));
        assert_eq!("running".parse(), Ok(JobStatus::Running));
        assert_eq!("succeeded".parse(), Ok(JobStatus::Succeeded));
        assert_eq!("failed".parse(), Ok(JobStatus::Failed));
        assert_eq!("unknown".parse::<JobStatus>(), Err(JobStatusParseError));
    }

    #[test]
    fn parses_supported_job_types() {
        assert_eq!("demo_delay".parse(), Ok(JobType::DemoDelay));
        assert_eq!("image_resize".parse(), Ok(JobType::ImageResize));
        assert_eq!("unknown".parse::<JobType>(), Err(JobTypeParseError));
    }
}
