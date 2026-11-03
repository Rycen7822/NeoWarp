#[cfg(feature = "local_fs")]
use futures::AsyncReadExt;
#[cfg(feature = "local_fs")]
use mime_guess::from_path;
use serde::{Deserialize, Serialize};
#[cfg(feature = "local_fs")]
use warp_files::{FileModel, TextFileReadResult};
#[cfg(feature = "local_fs")]
use warp_util::file::FileLoadError;
#[cfg(feature = "local_fs")]
use warp_util::file_type::is_buffer_binary;

#[cfg(feature = "local_fs")]
use crate::util::image::{ProcessImageResult, is_supported_image_mime_type, process_image};
#[cfg(feature = "local_fs")]
use crate::util::openable_file_type::is_binary_file;
#[cfg(feature = "local_fs")]
use crate::util::path::host_native_absolute_path;

pub mod external_editor;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileLocations {
    pub name: String,
    pub lines: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadFilesFailedFile {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum AnyFileContent {
    StringContent(String),
    BinaryContent(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct FileContext {
    pub file_name: String,
    pub content: AnyFileContent,
    pub line_range: Option<Range<usize>>,
    pub last_modified: Option<SystemTime>,
    pub line_count: usize,
}

impl FileContext {
    // create a new FileContext and autocalculate number of lines in the given file
    pub fn new(
        file_name: String,
        content: AnyFileContent,
        line_range: Option<Range<usize>>,
        last_modified: Option<SystemTime>,
    ) -> Self {
        let string_content = if let AnyFileContent::StringContent(content) = content.clone() {
            content
        } else {
            return Self {
                file_name,
                content,
                line_range,
                last_modified,
                line_count: 0,
            };
        };

        let line_count = string_content.lines().count();

        Self {
            file_name,
            content: AnyFileContent::StringContent(string_content),
            line_range,
            last_modified,
            line_count,
        }
    }
}

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

#[cfg(windows)]
use warp_util::path::is_network_resource;
use warp_util::path::{CleanPathResult, LineAndColumnArg};

pub use self::external_editor::{open_file_path_in_external_editor, open_file_path_with_editor};
use crate::terminal::ShellLaunchData;
use crate::terminal::model::grid::grid_handler::{ContainsPoint, Link};
use crate::terminal::model::index::Point;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePathType {
    Absolute,
    /// Contains the working directory PathBuf.
    Relative(PathBuf),
}

#[derive(Debug)]
pub enum ShellPathType {
    /// The path comes from the shell and may need to be converted in a shell-aware way.
    ShellNative(String),
    /// The path has already been converted to a OS-native path.
    PlatformNative(PathBuf),
}

/// Checks if a file path exists and is valid for a file link.
pub fn absolute_path_if_valid(
    clean_path_result: &CleanPathResult,
    working_directory: ShellPathType,
    shell_launch_data: Option<&ShellLaunchData>,
) -> Option<PathBuf> {
    // WSL/MSYS2 output can also contain paths that already belong to the Windows host.
    #[cfg(windows)]
    {
        let path = Path::new(&clean_path_result.path);
        if path.is_absolute() && is_path_valid(path, clean_path_result) {
            return Some(path.to_path_buf());
        }
    }
    let (maybe_absolute_path, relative_path) = match shell_launch_data {
        Some(shell_launch_data) => {
            // Attempt to parse the clean path result as an absolute path.
            let maybe_absolute_path =
                shell_launch_data.maybe_convert_absolute_path(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(base_path_str) => shell_launch_data
                    .maybe_convert_relative_path(&base_path_str, &clean_path_result.path),
                ShellPathType::PlatformNative(base_path) => {
                    shell_launch_data.join_to_native_path(&base_path, &clean_path_result.path)
                }
            };
            (maybe_absolute_path, relative_path)
        }
        None => {
            // We naively attempt to treat the given paths as platform-native.
            let maybe_absolute_path = PathBuf::from(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(path_str) => {
                    let mut path_buf = PathBuf::from(path_str);
                    path_buf.push(&clean_path_result.path);
                    path_buf
                }
                ShellPathType::PlatformNative(path_buf) => path_buf.join(&clean_path_result.path),
            };
            (Some(maybe_absolute_path), Some(relative_path))
        }
    };

    if relative_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result))
    {
        return relative_path;
    } else if maybe_absolute_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result))
    {
        return maybe_absolute_path;
    }

    None
}

fn is_path_valid(path: &Path, clean_path_result: &CleanPathResult) -> bool {
    // Checking for the existence of a network resource takes a long time (~15s),
    // and hangs the UI, so we skip validating it.
    #[cfg(windows)]
    if is_network_resource(path) {
        return false;
    }

    // It should only be a valid path if the path links to a file or a folder without
    // line and column number attached.
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() || (metadata.is_dir() && clean_path_result.line_and_column_num.is_none())
}

impl FilePathType {
    /// Given a path that we've identified the FilePathType of,
    /// returns the absolute path.
    pub fn absolute_path(&self, path: PathBuf) -> PathBuf {
        match self {
            FilePathType::Absolute => path,
            FilePathType::Relative(directory) => directory.join(&path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    pub link: Link,
    /// This path has been converted (if needed) into a native path from the shell.
    pub absolute_path: PathBuf,
    pub line_and_column_num: Option<LineAndColumnArg>,
}

impl FileLink {
    pub fn absolute_path(&self) -> Option<PathBuf> {
        Some(self.absolute_path.clone())
    }
}

impl ContainsPoint for FileLink {
    fn contains(&self, point: Point) -> bool {
        self.link.contains(point)
    }
}

/// Creates the file at the given path if it doesn't already exist, opening it
/// in write mode. If any directories in the path are missing, those are created
/// as well.
///
/// This always returns an error for unit tests, as they should not directly
/// interact with the filesystem.
pub fn create_file<P: AsRef<Path>>(_path: P) -> io::Result<fs::File> {
    cfg_if::cfg_if! {
        if #[cfg(test)] {
            Err(io::Error::from_raw_os_error(1))
        } else {
            let path = _path.as_ref();
            fs::create_dir_all(path.parent().ok_or_else(|| {
                io::Error::other(
                    "full_path should never be root directory.",
                )
            })?)?;
            fs::File::create(path)
        }
    }
}

/// Per-file byte limit for [`read_local_file_context`]. Binary files larger
/// than this are skipped; text files are truncated at this limit.
#[cfg(feature = "local_fs")]
const MAX_FILE_READ_BYTES: usize = 1_000_000;

/// The results of a [`read_local_file_context`] call.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadFileContextResult {
    /// [`FileContext`] data for all files that could be read.
    pub file_contexts: Vec<FileContext>,
    /// Requested files that could not be read, each paired with a reason-specific
    /// failure message (missing, too large, or unprocessable).
    pub failed_files: Vec<ReadFilesFailedFile>,
}

/// Reads the content of the given files at the given `FileLocations`.
///
/// If any files do not exist, they are included in the `failed_files` field of the result.
///
/// Binary files larger than the per-file byte limit are skipped and reported as
/// too large. Text files are truncated at the per-file limit via line streaming.
/// If `max_file_bytes` is provided, it overrides the default per-file limit
/// ([`MAX_FILE_READ_BYTES`]). Pass `None` to use the default.
/// If `max_batch_bytes` is provided, the cumulative content of all files is capped at that
/// budget; once exceeded, remaining files are reported as too large.
#[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
pub async fn read_local_file_context(
    file_names: &[FileLocations],
    current_working_directory: Option<String>,
    shell: Option<ShellLaunchData>,
    max_file_bytes: Option<usize>,
    max_batch_bytes: Option<usize>,
) -> anyhow::Result<ReadFileContextResult> {
    #[cfg(not(feature = "local_fs"))]
    return Err(anyhow::anyhow!(
        "Can't read files when not on a local filesystem"
    ));

    #[cfg(feature = "local_fs")]
    {
        let mut result = ReadFileContextResult {
            file_contexts: Vec::new(),
            failed_files: Vec::new(),
        };

        let mut batch_bytes_remaining = max_batch_bytes;

        for file in file_names {
            let absolute_file_path = PathBuf::from(host_native_absolute_path(
                &file.name,
                &shell,
                &current_working_directory,
            ));

            let metadata = match async_fs::metadata(&absolute_file_path).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    result.failed_files.push(ReadFilesFailedFile {
                        path: absolute_file_path.to_string_lossy().to_string(),
                        message: "File does not exist".to_string(),
                    });
                    continue;
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            };
            let last_modified = metadata.modified().ok();
            let file_size = metadata.len() as usize;
            let path_str = absolute_file_path.to_string_lossy().to_string();

            // Effective byte budget: the tighter of per-file and remaining batch.
            let per_file_limit = max_file_bytes.unwrap_or(MAX_FILE_READ_BYTES);
            let effective_max = match batch_bytes_remaining {
                Some(remaining) => per_file_limit.min(remaining),
                None => per_file_limit,
            };

            // Decide text vs binary before opening the file. Extension-based
            // detection alone is wrong for extensionless text files (e.g. shell
            // scripts named `bundle`, `run`), so for the ambiguous case we fall
            // back to content-based inspection of the first chunk. The binary
            // path below still acts as a safety net via
            // `TextFileReadResult::NotText` if the text reader trips on invalid
            // UTF-8.
            if !should_read_as_binary(&absolute_file_path).await {
                match FileModel::read_text_file(
                    &absolute_file_path,
                    effective_max,
                    &file.lines,
                    last_modified,
                )
                .await?
                {
                    TextFileReadResult::Segments {
                        segments,
                        bytes_read,
                    } => {
                        if let Some(remaining) = &mut batch_bytes_remaining {
                            *remaining = remaining.saturating_sub(bytes_read);
                        }
                        result
                            .file_contexts
                            .extend(segments.into_iter().map(|seg| FileContext {
                                file_name: seg.file_name,
                                content: AnyFileContent::StringContent(seg.content),
                                line_range: seg.line_range,
                                last_modified: seg.last_modified,
                                line_count: seg.line_count,
                            }));
                        continue;
                    }
                    TextFileReadResult::NotText => {
                        // Fall through to binary path below.
                    }
                }
            }

            // Binary path (either detected as binary, or text reading failed).
            match read_binary_file_context(
                &absolute_file_path,
                effective_max,
                file_size,
                last_modified,
            )
            .await?
            {
                BinaryFileReadResult::Context {
                    file_context,
                    bytes_read,
                } => {
                    if let Some(remaining) = &mut batch_bytes_remaining {
                        *remaining = remaining.saturating_sub(bytes_read);
                    }
                    result.file_contexts.push(file_context);
                }
                BinaryFileReadResult::NotFound => result.failed_files.push(ReadFilesFailedFile {
                    path: path_str,
                    message: "File does not exist".to_string(),
                }),
                BinaryFileReadResult::TooLarge {
                    size_bytes,
                    limit_bytes,
                } => result.failed_files.push(ReadFilesFailedFile {
                    path: path_str,
                    message: format!(
                        "File is too large to read ({} > {} limit). Downscale/compress it or read a smaller copy.",
                        format_mb(size_bytes),
                        format_mb(limit_bytes)
                    ),
                }),
                BinaryFileReadResult::ProcessingFailed { detail } => {
                    result.failed_files.push(ReadFilesFailedFile {
                        path: path_str,
                        message: format!("File could not be processed as an image: {detail}"),
                    })
                }
            }
        }

        Ok(result)
    }
}

/// Returns `true` if the file at `path` should be read via the binary code
/// path in [`read_local_file_context`], `false` if it should be read as text.
///
/// Uses extension-based detection as a fast path and falls back to content
/// inspection (reading the first 1 KiB) for extensionless files whose names
/// don't match any known text or binary pattern. Without the content-based
/// fallback, extensionless text files (e.g. shell scripts named `bundle`)
/// would be incorrectly classified as binary and returned to the agent as
/// raw bytes instead of UTF-8 text.
#[cfg(feature = "local_fs")]
async fn should_read_as_binary(path: &std::path::Path) -> bool {
    // Fast path: extension/filename clearly indicates text.
    if !is_binary_file(path) {
        return false;
    }
    // Fast path: file has a known binary extension (e.g. `.png`, `.exe`).
    if path.extension().is_some() {
        return true;
    }
    // Extensionless file with an unknown basename. Inspect the first chunk of
    // the file to decide. Treat open/read errors as binary so the binary path
    // takes over and reports a consistent error.
    is_file_content_binary_async(path).await
}

/// Async sibling of [`warp_util::file_type::is_file_content_binary`]. Reads
/// the first 1 KiB of `path` asynchronously and returns `true` if the content
/// looks binary according to [`is_buffer_binary`]. Returns `true` on any I/O
/// error so callers default to the binary code path. Kept local to this
/// module so `warp_util` doesn't need to grow an `async_fs` dependency.
#[cfg(feature = "local_fs")]
async fn is_file_content_binary_async(path: &std::path::Path) -> bool {
    const CHUNK_SIZE: usize = 1024;

    let Ok(mut file) = async_fs::File::open(path).await else {
        return true;
    };
    let mut buffer = [0u8; CHUNK_SIZE];
    let Ok(n) = file.read(&mut buffer).await else {
        return true;
    };
    is_buffer_binary(&buffer[..n])
}

/// Renders a byte count in megabytes with one decimal place (e.g. `3.5 MB`),
/// matching the units used in the "too large" failure message.
#[cfg(feature = "local_fs")]
fn format_mb(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[cfg(feature = "local_fs")]
enum BinaryFileReadResult {
    /// Successfully read as binary.
    Context {
        file_context: FileContext,
        bytes_read: usize,
    },
    /// The file does not exist on disk.
    NotFound,
    /// The file exists but exceeds the effective byte limit.
    TooLarge {
        size_bytes: usize,
        limit_bytes: usize,
    },
    /// The file could not be processed (e.g. an image decode/resize failed, or
    /// the processed image is still too large to send).
    ProcessingFailed { detail: String },
}

/// Reads a binary file, applying image processing when applicable.
#[cfg(feature = "local_fs")]
async fn read_binary_file_context(
    path: &std::path::Path,
    max_bytes: usize,
    file_size: usize,
    last_modified: Option<std::time::SystemTime>,
) -> anyhow::Result<BinaryFileReadResult> {
    if file_size > max_bytes {
        return Ok(BinaryFileReadResult::TooLarge {
            size_bytes: file_size,
            limit_bytes: max_bytes,
        });
    }

    let content = match read_file_as_binary(path).await {
        Ok(content) => content,
        Err(FileLoadError::DoesNotExist) => return Ok(BinaryFileReadResult::NotFound),
        Err(FileLoadError::IOError(e)) => return Err(anyhow::anyhow!(e)),
    };

    let mime_type = from_path(path).first_or_octet_stream().to_string();
    let processed_content = if is_supported_image_mime_type(&mime_type) {
        match process_image(&content) {
            ProcessImageResult::Success { data } => Some(data),
            ProcessImageResult::TooLarge => {
                log::warn!("Image file too large after processing: {}", path.display());
                return Ok(BinaryFileReadResult::ProcessingFailed {
                    detail: "the image is too large to send even after resizing".to_string(),
                });
            }
            ProcessImageResult::Error(err) => {
                log::warn!("Error processing image file {}: {err:?}", path.display());
                return Ok(BinaryFileReadResult::ProcessingFailed {
                    detail: err.to_string(),
                });
            }
        }
    } else {
        None
    };

    let final_content = processed_content.unwrap_or(content);
    if final_content.len() > max_bytes {
        return Ok(BinaryFileReadResult::TooLarge {
            size_bytes: final_content.len(),
            limit_bytes: max_bytes,
        });
    }

    let bytes_read = final_content.len();
    Ok(BinaryFileReadResult::Context {
        file_context: FileContext::new(
            path.to_string_lossy().to_string(),
            AnyFileContent::BinaryContent(final_content),
            None,
            last_modified,
        ),
        bytes_read,
    })
}

#[cfg(feature = "local_fs")]
async fn read_file_as_binary(file_path: &std::path::Path) -> Result<Vec<u8>, FileLoadError> {
    if !FileModel::file_exists(file_path).await {
        return Err(FileLoadError::DoesNotExist);
    }

    async_fs::read(file_path).await.map_err(FileLoadError::from)
}

#[cfg(all(test, feature = "local_fs"))]
mod file_tests;
