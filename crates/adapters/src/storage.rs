use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

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

    pub async fn ping(&self) -> Result<(), StorageError> {
        let probe = self.root.join(format!(".readiness-{}", Uuid::new_v4()));
        fs::write(&probe, []).await?;
        fs::remove_file(probe).await?;
        Ok(())
    }

    pub async fn usage_bytes(&self) -> Result<u64, StorageError> {
        let root = Arc::clone(&self.root);
        tokio::task::spawn_blocking(move || directory_usage(&root))
            .await
            .map_err(StorageError::Join)?
    }

    pub fn resolve_key(&self, key: &str) -> Result<PathBuf, StorageError> {
        let relative = Path::new(key);
        if key.is_empty()
            || key.contains('\\')
            || key.contains(':')
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

    pub async fn remove_file(&self, key: &str) -> Result<(), StorageError> {
        let path = self.resolve_key(key)?;
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn cleanup_unreferenced(
        &self,
        referenced_keys: HashSet<String>,
        protected_prefixes: Vec<String>,
        older_than: SystemTime,
    ) -> Result<CleanupReport, StorageError> {
        for key in referenced_keys.iter().chain(protected_prefixes.iter()) {
            self.resolve_key(key)?;
        }
        let root = Arc::clone(&self.root);
        tokio::task::spawn_blocking(move || {
            cleanup_directory(
                &root,
                &root,
                &referenced_keys,
                &protected_prefixes,
                older_than,
            )
        })
        .await
        .map_err(StorageError::Join)?
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub files_removed: u64,
    pub bytes_removed: u64,
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
    Join(tokio::task::JoinError),
    UsageOverflow,
}

impl Display for StorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "storage operation failed: {error}"),
            Self::InvalidKey => write!(formatter, "storage key is not a safe relative path"),
            Self::Join(error) => write!(formatter, "storage maintenance task failed: {error}"),
            Self::UsageOverflow => write!(formatter, "storage usage exceeds the supported range"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidKey => None,
            Self::Join(error) => Some(error),
            Self::UsageOverflow => None,
        }
    }
}

fn directory_usage(root: &Path) -> Result<u64, StorageError> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .ok_or(StorageError::UsageOverflow)?;
            }
        }
    }
    Ok(total)
}

fn cleanup_directory(
    root: &Path,
    directory: &Path,
    referenced_keys: &HashSet<String>,
    protected_prefixes: &[String],
    older_than: SystemTime,
) -> Result<CleanupReport, StorageError> {
    let mut report = CleanupReport::default();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let child =
                cleanup_directory(root, &path, referenced_keys, protected_prefixes, older_than)?;
            report.files_removed += child.files_removed;
            report.bytes_removed += child.bytes_removed;
            if directory != root && std::fs::read_dir(&path)?.next().is_none() {
                std::fs::remove_dir(&path)?;
            }
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|_| StorageError::InvalidKey)?
            .to_string_lossy()
            .replace('\\', "/");
        let protected = referenced_keys.contains(&relative)
            || protected_prefixes
                .iter()
                .any(|prefix| relative == *prefix || relative.starts_with(&format!("{prefix}/")));
        let old_enough = metadata
            .modified()
            .is_ok_and(|modified| modified <= older_than);
        if !protected && old_enough {
            std::fs::remove_file(path)?;
            report.files_removed += 1;
            report.bytes_removed = report
                .bytes_removed
                .checked_add(metadata.len())
                .ok_or(StorageError::UsageOverflow)?;
        }
    }
    Ok(report)
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    use super::LocalStorage;

    #[tokio::test]
    async fn confines_keys_and_preserves_references_and_active_attempts() {
        let temporary = tempfile::tempdir().expect("temporary storage should be created");
        let storage = LocalStorage::initialize(temporary.path())
            .await
            .expect("storage should initialize");
        assert!(storage.resolve_key("../outside").is_err());
        assert!(storage.resolve_key("inputs/../../outside").is_err());
        assert!(storage.resolve_key("C:\\outside").is_err());
        assert!(storage.resolve_key("C:/outside").is_err());
        assert!(storage.resolve_key("\\\\server\\share").is_err());

        for (key, contents) in [
            ("inputs/referenced/source.png", b"input".as_slice()),
            ("inputs/orphan/source.png", b"orphan".as_slice()),
            ("outputs/job-2/attempt-8/partial.jpg", b"active".as_slice()),
            ("outputs/job-1/attempt-3/old.jpg", b"old".as_slice()),
        ] {
            let path = storage.resolve_key(key).expect("test key should be safe");
            tokio::fs::create_dir_all(path.parent().expect("file should have a parent"))
                .await
                .expect("test directory should be created");
            tokio::fs::write(path, contents)
                .await
                .expect("test file should be written");
        }

        let report = storage
            .cleanup_unreferenced(
                HashSet::from(["inputs/referenced/source.png".to_owned()]),
                vec!["outputs/job-2/attempt-8".to_owned()],
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
            .expect("cleanup should succeed");

        assert_eq!(report.files_removed, 2);
        assert!(
            storage
                .resolve_key("inputs/referenced/source.png")
                .expect("key should resolve")
                .exists()
        );
        assert!(
            storage
                .resolve_key("outputs/job-2/attempt-8/partial.jpg")
                .expect("key should resolve")
                .exists()
        );
        assert!(
            !storage
                .resolve_key("inputs/orphan/source.png")
                .expect("key should resolve")
                .exists()
        );
    }
}
