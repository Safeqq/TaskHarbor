use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

pub const MAX_JOB_NAME_LENGTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

impl JobId {
    pub const fn new(value: u64) -> Result<Self, JobIdError> {
        if value == 0 {
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
        write!(formatter, "job ID must be a positive integer")
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    name: JobName,
    status: JobStatus,
}

impl Job {
    pub const fn new_queued(id: JobId, name: JobName) -> Self {
        Self {
            id,
            name,
            status: JobStatus::Queued,
        }
    }

    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn status(&self) -> JobStatus {
        self.status
    }

    pub fn name(&self) -> &JobName {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::{Job, JobId, JobIdError, JobName, JobNameError, JobStatus, MAX_JOB_NAME_LENGTH};

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
    fn creates_a_job_in_the_queued_state() {
        let id = JobId::new(1).expect("test ID should be valid");
        let name = JobName::new("resize profile images").expect("test name should be valid");

        let job = Job::new_queued(id, name);

        assert_eq!(job.id(), id);
        assert_eq!(job.status(), JobStatus::Queued);
    }
}
