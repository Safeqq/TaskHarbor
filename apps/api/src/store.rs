use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use taskharbor_core::{Job, JobId, JobName};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct MemoryJobStore {
    inner: Arc<RwLock<StoreData>>,
}

impl MemoryJobStore {
    pub async fn create(&self, name: JobName) -> Result<Job, StoreError> {
        let mut data = self.inner.write().await;
        let id = JobId::new(data.next_id).map_err(|_| StoreError::IdExhausted)?;
        data.next_id = data.next_id.checked_add(1).ok_or(StoreError::IdExhausted)?;

        let job = Job::new_queued(id, name);
        data.jobs.insert(id, job.clone());

        Ok(job)
    }

    pub async fn list(&self) -> Vec<Job> {
        let data = self.inner.read().await;
        data.jobs.values().cloned().collect()
    }

    pub async fn get(&self, id: JobId) -> Option<Job> {
        let data = self.inner.read().await;
        data.jobs.get(&id).cloned()
    }
}

impl Default for MemoryJobStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StoreData::default())),
        }
    }
}

#[derive(Debug)]
struct StoreData {
    next_id: u64,
    jobs: BTreeMap<JobId, Job>,
}

impl Default for StoreData {
    fn default() -> Self {
        Self {
            next_id: 1,
            jobs: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    IdExhausted,
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdExhausted => write!(formatter, "no job IDs remain available"),
        }
    }
}

impl Error for StoreError {}
