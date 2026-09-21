use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits, Rgb, RgbImage};
use tokio::sync::Semaphore;
use tokio::task::{JoinError, spawn_blocking};

use crate::lifecycle_repository::FailureKind;
use crate::{LocalStorage, StorageError};

pub const MAX_FILES_PER_JOB: usize = 10;
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_TOTAL_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 12_000_000;
pub const MAX_IMAGE_DIMENSION: u32 = 12_000;
pub const MAX_OUTPUT_WIDTH: u32 = 8_192;
pub const DEFAULT_OUTPUT_WIDTH: u32 = 1_600;
pub const DEFAULT_JPEG_QUALITY: u8 = 85;
const MAX_DECODE_ALLOC_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageMetadata {
    pub media_type: &'static str,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedImage {
    pub storage_key: String,
    pub media_type: &'static str,
    pub byte_size: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct ImageService {
    storage: LocalStorage,
    blocking_limit: Arc<Semaphore>,
}

impl ImageService {
    pub fn new(storage: LocalStorage, max_blocking_tasks: usize) -> Self {
        Self {
            storage,
            blocking_limit: Arc::new(Semaphore::new(max_blocking_tasks.max(1))),
        }
    }

    pub async fn inspect(&self, storage_key: &str) -> Result<ImageMetadata, ImageError> {
        let path = self.storage.resolve_key(storage_key)?;
        let permit = self
            .blocking_limit
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ImageError::BlockingLimitClosed)?;

        spawn_blocking(move || {
            let _permit = permit;
            inspect_path(&path)
        })
        .await?
    }

    pub async fn resize_to_jpeg(
        &self,
        input_key: &str,
        output_key: String,
        max_width: u32,
        jpeg_quality: u8,
    ) -> Result<ProcessedImage, ImageError> {
        if max_width == 0 || max_width > MAX_OUTPUT_WIDTH || !(1..=100).contains(&jpeg_quality) {
            return Err(ImageError::InvalidSettings);
        }

        let input_path = self.storage.resolve_key(input_key)?;
        let output_path = self.storage.resolve_key(&output_key)?;
        let permit = self
            .blocking_limit
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ImageError::BlockingLimitClosed)?;

        spawn_blocking(move || {
            let _permit = permit;
            process_path(
                &input_path,
                &output_path,
                output_key,
                max_width,
                jpeg_quality,
            )
        })
        .await?
    }
}

fn inspect_path(path: &Path) -> Result<ImageMetadata, ImageError> {
    let reader = BufReader::new(File::open(path)?);
    let mut reader = ImageReader::new(reader).with_guessed_format()?;
    let format = reader.format().ok_or(ImageError::UnsupportedFormat)?;

    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png) {
        return Err(ImageError::UnsupportedFormat);
    }

    reader.limits(decode_limits());
    let (width, height) = reader.into_dimensions()?;
    validate_dimensions(width, height)?;

    if format == ImageFormat::Png {
        let decoder = PngDecoder::with_limits(BufReader::new(File::open(path)?), decode_limits())?;
        if decoder.is_apng()? {
            return Err(ImageError::AnimatedPng);
        }
    }

    Ok(ImageMetadata {
        media_type: match format {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            _ => return Err(ImageError::UnsupportedFormat),
        },
        width,
        height,
    })
}

fn process_path(
    input_path: &Path,
    output_path: &Path,
    output_key: String,
    max_width: u32,
    jpeg_quality: u8,
) -> Result<ProcessedImage, ImageError> {
    let metadata = inspect_path(input_path)?;
    let reader = BufReader::new(File::open(input_path)?);
    let mut reader = ImageReader::new(reader).with_guessed_format()?;
    reader.limits(decode_limits());
    let decoded = reader.decode()?;
    let flattened = flatten_on_white(&decoded);
    let (output_width, output_height) =
        resized_dimensions(metadata.width, metadata.height, max_width);
    let output = if output_width == metadata.width && output_height == metadata.height {
        flattened
    } else {
        image::imageops::resize(
            &flattened,
            output_width,
            output_height,
            FilterType::Lanczos3,
        )
    };

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let encode_result = (|| -> Result<(), ImageError> {
        let file = File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        JpegEncoder::new_with_quality(&mut writer, jpeg_quality).encode_image(&output)?;
        writer.flush()?;
        Ok(())
    })();

    if let Err(error) = encode_result {
        let _ = fs::remove_file(output_path);
        return Err(error);
    }

    let byte_size = fs::metadata(output_path)?.len();
    Ok(ProcessedImage {
        storage_key: output_key,
        media_type: "image/jpeg",
        byte_size,
        width: output_width,
        height: output_height,
    })
}

fn flatten_on_white(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y).0;
        let alpha = u16::from(pixel[3]);
        let blend = |channel: u8| {
            let foreground = u16::from(channel) * alpha;
            let background = 255 * (255 - alpha);
            ((foreground + background + 127) / 255) as u8
        };
        Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])])
    })
}

fn resized_dimensions(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    if width <= max_width {
        return (width, height);
    }

    let scaled_height =
        (u64::from(height) * u64::from(max_width) + u64::from(width / 2)) / u64::from(width);
    (
        max_width,
        u32::try_from(scaled_height).unwrap_or(u32::MAX).max(1),
    )
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), ImageError> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ImageError::DimensionsTooLarge)?;

    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || pixels > MAX_IMAGE_PIXELS
    {
        return Err(ImageError::DimensionsTooLarge);
    }

    Ok(())
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    limits
}

#[derive(Debug)]
pub enum ImageError {
    Io(std::io::Error),
    Decode(image::ImageError),
    Storage(StorageError),
    Join(JoinError),
    UnsupportedFormat,
    AnimatedPng,
    DimensionsTooLarge,
    InvalidSettings,
    BlockingLimitClosed,
}

impl ImageError {
    pub const fn is_invalid_input(&self) -> bool {
        matches!(
            self,
            Self::Decode(_)
                | Self::UnsupportedFormat
                | Self::AnimatedPng
                | Self::DimensionsTooLarge
        )
    }

    pub const fn safe_message(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "only JPEG and PNG image data is supported",
            Self::AnimatedPng => "animated PNG files are not supported",
            Self::DimensionsTooLarge => {
                "image dimensions exceed the 12 megapixel or 12000 pixel limit"
            }
            Self::Decode(_) => "image data is corrupt or could not be decoded",
            Self::InvalidSettings => "image resize settings are invalid",
            Self::Io(_) | Self::Storage(_) | Self::Join(_) | Self::BlockingLimitClosed => {
                "image processing could not be completed"
            }
        }
    }

    pub const fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Decode(_)
            | Self::UnsupportedFormat
            | Self::AnimatedPng
            | Self::DimensionsTooLarge
            | Self::InvalidSettings
            | Self::Storage(StorageError::InvalidKey) => FailureKind::Permanent,
            Self::Io(_)
            | Self::Storage(StorageError::Io(_))
            | Self::Storage(StorageError::Join(_) | StorageError::UsageOverflow)
            | Self::Join(_)
            | Self::BlockingLimitClosed => FailureKind::Transient,
        }
    }
}

impl Display for ImageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "image file operation failed: {error}"),
            Self::Decode(error) => write!(formatter, "image codec failed: {error}"),
            Self::Storage(error) => error.fmt(formatter),
            Self::Join(error) => write!(formatter, "blocking image task failed: {error}"),
            Self::UnsupportedFormat => write!(formatter, "image format is not JPEG or PNG"),
            Self::AnimatedPng => write!(formatter, "animated PNG is not supported"),
            Self::DimensionsTooLarge => {
                write!(formatter, "image dimensions exceed resource limits")
            }
            Self::InvalidSettings => write!(formatter, "image resize settings are invalid"),
            Self::BlockingLimitClosed => write!(formatter, "blocking task semaphore is closed"),
        }
    }
}

impl Error for ImageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Join(error) => Some(error),
            Self::UnsupportedFormat
            | Self::AnimatedPng
            | Self::DimensionsTooLarge
            | Self::InvalidSettings
            | Self::BlockingLimitClosed => None,
        }
    }
}

impl From<std::io::Error> for ImageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<image::ImageError> for ImageError {
    fn from(error: image::ImageError) -> Self {
        Self::Decode(error)
    }
}

impl From<StorageError> for ImageError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<JoinError> for ImageError {
    fn from(error: JoinError) -> Self {
        Self::Join(error)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::codecs::png::PngEncoder;
    use image::{ColorType, GenericImageView, ImageEncoder, ImageReader, Rgba, RgbaImage};

    use super::ImageService;
    use crate::{FailureKind, LocalStorage};

    #[tokio::test]
    async fn detects_content_resizes_without_upscaling_and_flattens_alpha() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let storage = LocalStorage::initialize(temporary.path())
            .await
            .expect("storage should initialize");
        let input_key = "inputs/test/content-without-extension";
        let input_path = storage
            .resolve_key(input_key)
            .expect("test key should be safe");
        fs::create_dir_all(
            input_path
                .parent()
                .expect("test input should have a parent"),
        )
        .expect("test input directory should be created");
        let rgba = RgbaImage::from_pixel(4, 2, Rgba([240, 20, 20, 0]));
        PngEncoder::new(fs::File::create(&input_path).expect("test input should be created"))
            .write_image(rgba.as_raw(), 4, 2, ColorType::Rgba8.into())
            .expect("test PNG should encode");

        let images = ImageService::new(storage.clone(), 1);
        let metadata = images
            .inspect(input_key)
            .await
            .expect("PNG content should be detected without an extension");
        assert_eq!(metadata.media_type, "image/png");
        assert_eq!((metadata.width, metadata.height), (4, 2));

        let resized = images
            .resize_to_jpeg(input_key, "outputs/test/resized.jpg".into(), 2, 90)
            .await
            .expect("test image should resize");
        assert_eq!((resized.width, resized.height), (2, 1));
        let decoded = ImageReader::open(
            storage
                .resolve_key(&resized.storage_key)
                .expect("output key should be safe"),
        )
        .expect("output should open")
        .decode()
        .expect("output JPEG should decode");
        assert_eq!(decoded.dimensions(), (2, 1));
        let pixel = decoded.to_rgb8().get_pixel(0, 0).0;
        assert!(pixel.iter().all(|channel| *channel >= 245));

        let unchanged = images
            .resize_to_jpeg(input_key, "outputs/test/not-upscaled.jpg".into(), 100, 90)
            .await
            .expect("small image should still encode");
        assert_eq!((unchanged.width, unchanged.height), (4, 2));
    }

    #[tokio::test]
    async fn rejects_non_image_content_and_unsafe_storage_keys() {
        let temporary = tempfile::tempdir().expect("temporary directory should be created");
        let storage = LocalStorage::initialize(temporary.path())
            .await
            .expect("storage should initialize");
        let input_key = "inputs/test/not-an-image";
        let input_path = storage
            .resolve_key(input_key)
            .expect("test key should be safe");
        fs::create_dir_all(
            input_path
                .parent()
                .expect("test input should have a parent"),
        )
        .expect("test input directory should be created");
        fs::write(input_path, b"this is not an image")
            .expect("invalid test input should be written");
        let images = ImageService::new(storage.clone(), 1);

        let error = images
            .inspect(input_key)
            .await
            .expect_err("non-image content should be rejected");
        assert!(error.is_invalid_input());
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
        let missing = images
            .inspect("inputs/test/missing-image")
            .await
            .expect_err("missing input should be an I/O failure");
        assert_eq!(missing.failure_kind(), FailureKind::Transient);
        assert!(storage.resolve_key("../outside").is_err());
        assert!(storage.resolve_key("/absolute").is_err());
    }
}
