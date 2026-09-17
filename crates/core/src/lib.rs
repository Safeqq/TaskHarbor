use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub const MAX_JOB_NAME_LENGTH: usize = 100;

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

#[cfg(test)]
mod tests {
    use super::{JobName, JobNameError, MAX_JOB_NAME_LENGTH};

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
}
