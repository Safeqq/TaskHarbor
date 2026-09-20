use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScheduleId(u64);

impl ScheduleId {
    pub const fn new(value: u64) -> Result<Self, ScheduleIdError> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(ScheduleIdError);
        }

        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Display for ScheduleId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ScheduleId {
    type Err = ScheduleIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.parse::<u64>().map_err(|_| ScheduleIdError)?;
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleIdError;

impl Display for ScheduleIdError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "schedule ID must be a positive signed 64-bit integer"
        )
    }
}

impl Error for ScheduleIdError {}

#[cfg(test)]
mod tests {
    use super::{ScheduleId, ScheduleIdError};

    #[test]
    fn validates_schedule_ids() {
        assert_eq!(ScheduleId::new(1).map(ScheduleId::get), Ok(1));
        assert_eq!(ScheduleId::new(0), Err(ScheduleIdError));
        assert_eq!(ScheduleId::new(u64::MAX), Err(ScheduleIdError));
    }
}
