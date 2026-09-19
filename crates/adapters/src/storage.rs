use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tokio::fs::{self, File};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct LocalStorage {
    root: Arc<PathBuf>,
}

impl LocalStorage {
    pub async fn initialize(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        fs::create_dir_all(root.as_ref()).await?;
        let root = fs::canonicalize(root.as_ref()).await?;
        fs::create_dir_all(root.join("inputs")).await?;
        fs::create_dir_all(root.join("outputs")).await?;

        Ok(Self {
            root: Arc::new(root),
        })
    }

    pub async fn begin_upload(&self) -> Result<UploadBatch, StorageError> {
        let prefix = format!("inputs/{}", Uuid::new_v4());
        fs::create_dir_all(self.resolve_key(&prefix)?).await?;

        Ok(UploadBatch {
            storage: self.clone(),
            prefix,
        })
    }

    pub fn output_key(&self, job_id: u64, attempt_id: i64, input_id: u64) -> String {
        format!(
            "{}/input-{input_id}.jpg",
            self.attempt_output_prefix(job_id, attempt_id)
        )
    }

    pub fn attempt_output_prefix(&self, job_id: u64, attempt_id: i64) -> String {
        format!("outputs/job-{job_id}/attempt-{attempt_id}")
    }

    pub async fn open(&self, key: &str) -> Result<File, StorageError> {
        Ok(File::open(self.resolve_key(key)?).await?)
    }

    pub fn resolve_key(&self, key: &str) -> Result<PathBuf, StorageError> {
        let relative = Path::new(key);
        if key.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(StorageError::InvalidKey);
        }

        Ok(self.root.join(relative))
    }

    pub async fn remove_tree(&self, key: &str) -> Result<(), StorageError> {
        let path = self.resolve_key(key)?;
        match fs::remove_dir_all(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug)]
pub struct UploadBatch {
    storage: LocalStorage,
    prefix: String,
}

impl UploadBatch {
    pub async fn create_file(&self) -> Result<(String, File), StorageError> {
        let key = format!("{}/{}", self.prefix, Uuid::new_v4());
        let file = File::create(self.storage.resolve_key(&key)?).await?;
        Ok((key, file))
    }

    pub async fn cleanup(&self) -> Result<(), StorageError> {
        self.storage.remove_tree(&self.prefix).await
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    InvalidKey,
}

impl Display for StorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "storage operation failed: {error}"),
            Self::InvalidKey => write!(formatter, "storage key is not a safe relative path"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidKey => None,
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
