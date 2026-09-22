//! Images the user gives the AI: pictures attached to one message, and the
//! project's reference library those can be kept in.
//!
//! An attachment belongs to the message it was sent with. Its bytes are
//! written once to `.cadmark/attachments/` and the conversation records
//! the file name, so reopening the project or replaying the conversation
//! finds the picture again without the conversation file carrying it.
//!
//! The reference library is `references/` in the project folder: a
//! deliberate, longer-lived home for the pictures a project is built from,
//! shared by every conversation. The AI reads and lists it through a tool,
//! guided by `references/INDEX.md`, where it writes a line per file saying
//! what the picture shows and what it is for. The user can also drop files
//! there by hand; they are listed without a description until the AI
//! describes them.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cadmark_core::message::{ImageAttachment, ImageData};

/// Where a message's images are kept, inside the project folder.
const ATTACHMENTS_DIRECTORY: &str = ".cadmark/attachments";
/// The project's reference library.
pub const REFERENCES_DIRECTORY: &str = "references";
/// The AI-maintained catalogue of the reference library.
pub const INDEX_FILENAME: &str = "INDEX.md";
/// The largest file the store accepts, before decoding: a photograph from
/// a phone is a few megabytes, and a provider rejects far less than this.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// A picture the user has staged in the chat input, decoded once so the
/// pane can show it and the store can keep the original bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedImage {
    /// The name the user knows it by: the file name it came from, or
    /// a label for a pasted picture.
    pub name: String,
    pub data: ImageData,
    pub thumbnail: egui::ColorImage,
    /// The extension the stored copy takes.
    extension: &'static str,
}

impl StagedImage {
    /// Decode a file the user chose. The format is read from the bytes,
    /// not the name: a download saved without an extension is still the
    /// picture it is.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let read = || -> Result<Self, String> {
            let size = std::fs::metadata(path)
                .map_err(|error| error.to_string())?
                .len();
            if size > MAX_IMAGE_BYTES {
                return Err(format!(
                    "{} MB is larger than the {} MB an image may be",
                    size / (1024 * 1024),
                    MAX_IMAGE_BYTES / (1024 * 1024)
                ));
            }
            let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
            Self::from_bytes(name.clone(), bytes)
        };
        read().map_err(|error| format!("{name}: {error}"))
    }

    /// Decode encoded bytes whose format is read from their header.
    pub fn from_bytes(name: String, bytes: Vec<u8>) -> Result<Self, String> {
        let format = image::guess_format(&bytes).map_err(|_| "not a PNG or JPEG image")?;
        let (media_type, extension) = match format {
            image::ImageFormat::Png => ("image/png", "png"),
            image::ImageFormat::Jpeg => ("image/jpeg", "jpg"),
            other => {
                return Err(format!(
                    "{other:?} images are not supported; use PNG or JPEG"
                ));
            }
        };
        // ImageReader's allocation limits apply before decoding a photograph.
        let decoded = image::ImageReader::with_format(Cursor::new(&bytes), format)
            .decode()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            name,
            thumbnail: thumbnail_of(&decoded),
            data: ImageData {
                media_type: media_type.into(),
                bytes,
            },
            extension,
        })
    }

    /// Encode raw pixels from the clipboard as a PNG.
    pub fn from_rgba(
        name: String,
        width: usize,
        height: usize,
        rgba: &[u8],
    ) -> Result<Self, String> {
        let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
            .ok_or("the clipboard image's pixels do not match its size")?;
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        Self::from_bytes(name, bytes)
    }
}

fn thumbnail_of(decoded: &image::DynamicImage) -> egui::ColorImage {
    let thumbnail = decoded.thumbnail(96, 96).to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied(
        [thumbnail.width() as usize, thumbnail.height() as usize],
        thumbnail.as_raw(),
    )
}

/// A file name that is safe inside the project folder and unlikely to
/// collide: the moment it was stored, then the user's name for it.
fn stored_name(name: &str, extension: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let mut safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    safe.truncate(48);
    let safe = safe.trim_matches('-');
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    if safe.is_empty() {
        format!("{stamp}.{extension}")
    } else {
        format!("{stamp}-{safe}.{extension}")
    }
}

/// Write `bytes` into `directory` under a name based on `wanted`, never
/// replacing an existing file. Returns the name used.
fn write_new_file(directory: &Path, wanted: &str, bytes: &[u8]) -> Result<String, String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    let (stem, extension) = match wanted.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension),
        None => (wanted, ""),
    };
    for suffix in 0.. {
        let name = match (suffix, extension.is_empty()) {
            (0, _) => wanted.to_string(),
            (_, true) => format!("{stem}-{suffix}"),
            (_, false) => format!("{stem}-{suffix}.{extension}"),
        };
        match temporary.persist_noclobber(directory.join(&name)) {
            Ok(_) => return Ok(name),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file
            }
            Err(error) => return Err(error.error.to_string()),
        }
    }
    unreachable!()
}

/// Keep a staged image with the message it is sent on. The bytes go to the
/// attachment store; the returned attachment carries them too, so the turn
/// that is about to start reads them from memory.
pub fn store_attachment(project: &Path, staged: &StagedImage) -> Result<ImageAttachment, String> {
    let file = write_new_file(
        &project.join(ATTACHMENTS_DIRECTORY),
        &stored_name(&staged.name, staged.extension),
        &staged.data.bytes,
    )
    .map_err(|error| format!("Could not keep {}: {error}", staged.name))?;
    Ok(ImageAttachment {
        file,
        name: staged.name.clone(),
        media_type: staged.data.media_type.clone(),
        bytes: staged.data.bytes.clone(),
    })
}

/// Fill in the bytes of every attachment in a conversation loaded from
/// disk. An attachment whose file is gone stays empty: the message still
/// shows it by name, and the model is not sent it.
pub fn load_attachment_bytes(project: &Path, attachments: &mut [ImageAttachment]) {
    let directory = project.join(ATTACHMENTS_DIRECTORY);
    for attachment in attachments {
        if !attachment.bytes.is_empty() || !is_plain_file_name(&attachment.file) {
            continue;
        }
        match std::fs::read(directory.join(&attachment.file)) {
            Ok(bytes) => attachment.bytes = bytes,
            Err(error) => log::warn!("Attachment {} is not readable: {error}", attachment.file),
        }
    }
}

/// A name that stays inside one directory: no separators, no traversal.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.contains('\0')
}

// ── The reference library ─────────────────────────────────────────────

/// A file in the reference library with its catalogue line, if the AI has
/// written one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceEntry {
    pub file: String,
    pub description: Option<String>,
}

/// The reference library as the AI is shown it.
pub struct ReferenceLibrary {
    directory: PathBuf,
}

impl ReferenceLibrary {
    pub fn of_project(project: &Path) -> Self {
        Self {
            directory: project.join(REFERENCES_DIRECTORY),
        }
    }

    /// Every image in the library, with its description from the index.
    /// Files the index describes but which are gone are omitted.
    pub fn entries(&self) -> Result<Vec<ReferenceEntry>, String> {
        let mut files = Vec::new();
        match std::fs::read_dir(&self.directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let path = entry.path();
                    if is_image_extension(&path) {
                        files.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        files.sort();
        let index = self.read_index();
        Ok(files
            .into_iter()
            .map(|file| {
                let description = index
                    .iter()
                    .find(|(name, _)| name == &file)
                    .map(|(_, description)| description.clone());
                ReferenceEntry { file, description }
            })
            .collect())
    }

    /// The index as `(file, description)` lines, in file order.
    fn read_index(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(self.directory.join(INDEX_FILENAME))
            .map(|text| parse_index(&text))
            .unwrap_or_default()
    }

    /// Read one image, by its file name.
    pub fn read(&self, file: &str) -> Result<ImageData, String> {
        if !is_plain_file_name(file) {
            return Err(format!(
                "{file} is not a file name in the reference library"
            ));
        }
        let path = self.directory.join(file);
        if !is_image_extension(&path) {
            return Err(format!("{file} is not a PNG or JPEG image"));
        }
        let staged = StagedImage::from_file(&path)?;
        Ok(staged.data)
    }

    /// Copy an attachment into the library under `file`, and record
    /// `description` for it in the index. `file` is the name the AI chose;
    /// its extension follows the image's format regardless.
    pub fn keep(&self, image: &ImageData, file: &str, description: &str) -> Result<String, String> {
        if !is_plain_file_name(file) {
            return Err(format!("{file} is not a file name"));
        }
        let extension = match image.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            other => return Err(format!("{other} images cannot be kept")),
        };
        let stem = Path::new(file)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let wanted = format!("{stem}.{extension}");
        let name = write_new_file(&self.directory, &wanted, &image.bytes)?;
        self.describe(&name, description)?;
        Ok(name)
    }

    /// Write or replace the index line for `file`. The index keeps one
    /// line per file, in the order the files were first described.
    pub fn describe(&self, file: &str, description: &str) -> Result<(), String> {
        if !is_plain_file_name(file) {
            return Err(format!("{file} is not a file name"));
        }
        if !self.directory.join(file).is_file() {
            return Err(format!("{file} is not in the reference library"));
        }
        let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
        if description.is_empty() {
            return Err("a description is needed".into());
        }
        let mut entries = self.read_index();
        match entries.iter_mut().find(|(name, _)| name == file) {
            Some(entry) => entry.1 = description,
            None => entries.push((file.to_string(), description)),
        }
        std::fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        std::fs::write(self.directory.join(INDEX_FILENAME), render_index(&entries))
            .map_err(|error| error.to_string())
    }
}

fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg"
            )
        })
}

/// Index lines are `- \`file\`: description`; anything else in the file is
/// prose the user wrote and is left alone by `render_index`'s header.
fn parse_index(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("- `")?;
            let (file, description) = rest.split_once("`:")?;
            Some((file.to_string(), description.trim().to_string()))
        })
        .collect()
}

fn render_index(entries: &[(String, String)]) -> String {
    let mut text = String::from(
        "# Reference images\n\nWhat each picture in this folder shows and what it is for. \
         CADmark's AI keeps this list up to date; one line per file.\n\n",
    );
    for (file, description) in entries {
        text.push_str(&format!("- `{file}`: {description}\n"));
    }
    text
}

// ── The file picker ───────────────────────────────────────────────────

/// What the picker produced: the images it could decode, and why the
/// others could not be.
pub struct PickedImages {
    pub images: Vec<StagedImage>,
    pub errors: Vec<String>,
}

/// The picker and image decoding run outside the UI thread.
#[derive(Default)]
pub struct ImagePicker {
    receiver: Option<mpsc::Receiver<Option<PickedImages>>>,
}

impl ImagePicker {
    pub fn pending(&self) -> bool {
        self.receiver.is_some()
    }

    /// Open the system picker over the given folder. The picker offers
    /// every file: the format is read from the bytes when chosen, so a
    /// picture saved without an extension is not hidden.
    pub fn open(
        &mut self,
        frame: &eframe::Frame,
        ctx: &egui::Context,
        start_in: PathBuf,
    ) -> Result<(), String> {
        if self.pending() {
            return Ok(());
        }
        let dialog = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title("Attach images to your message")
            .set_directory(start_in)
            .add_filter("All files", &["*"])
            .add_filter(
                "PNG and JPEG images",
                &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"],
            );
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("cadmark-image-picker".into())
            .spawn(move || {
                let result = dialog.pick_files().map(|files| {
                    let mut images = Vec::new();
                    let mut errors = Vec::new();
                    for file in files {
                        match StagedImage::from_file(&file) {
                            Ok(image) => images.push(image),
                            Err(error) => errors.push(error),
                        }
                    }
                    PickedImages { images, errors }
                });
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .map_err(|error| format!("Could not open the image picker: {error}"))?;
        self.receiver = Some(receiver);
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<PickedImages>, String> {
        let Some(receiver) = &self.receiver else {
            return Ok(None);
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver = None;
                Ok(result)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Err("The image picker stopped before reporting its result".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32, colour: [u8; 3]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(width, height, image::Rgb(colour))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn a_file_without_an_extension_is_read_by_its_bytes() {
        let sources = tempfile::tempdir().unwrap();
        let path = sources.path().join("Flange");
        std::fs::write(&path, png_bytes(480, 240, [20, 80, 160])).unwrap();
        let staged = StagedImage::from_file(&path).unwrap();
        assert_eq!(staged.name, "Flange");
        assert_eq!(staged.data.media_type, "image/png");
        assert_eq!(staged.thumbnail.size, [96, 48]);

        let jpeg = sources.path().join("photo.PNG");
        image::RgbImage::from_pixel(4, 2, image::Rgb([1, 2, 3]))
            .save_with_format(&jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let staged = StagedImage::from_file(&jpeg).unwrap();
        assert_eq!(
            staged.data.media_type, "image/jpeg",
            "the bytes decide, not the name"
        );

        std::fs::write(&path, b"not an image").unwrap();
        assert!(
            StagedImage::from_file(&path)
                .unwrap_err()
                .contains("Flange")
        );
    }

    #[test]
    fn pasted_pixels_become_a_png() {
        let staged = StagedImage::from_rgba(
            "Pasted image".into(),
            2,
            1,
            &[255, 0, 0, 255, 0, 0, 255, 255],
        )
        .unwrap();
        assert_eq!(staged.data.media_type, "image/png");
        assert_eq!(
            image::guess_format(&staged.data.bytes).unwrap(),
            image::ImageFormat::Png
        );
        assert!(StagedImage::from_rgba("x".into(), 3, 3, &[0; 4]).is_err());
    }

    #[test]
    fn attachments_are_kept_out_of_the_way_and_reload_by_name() {
        let project = tempfile::tempdir().unwrap();
        let staged =
            StagedImage::from_bytes("my flange.png".into(), png_bytes(4, 2, [0, 80, 160])).unwrap();
        let first = store_attachment(project.path(), &staged).unwrap();
        let second = store_attachment(project.path(), &staged).unwrap();
        assert_ne!(first.file, second.file);
        assert!(first.file.ends_with("-my-flange.png"), "{}", first.file);
        assert_eq!(first.name, "my flange.png");
        assert_eq!(first.bytes, staged.data.bytes);
        assert!(
            project
                .path()
                .join(".cadmark/attachments")
                .join(&first.file)
                .is_file()
        );
        assert!(!project.path().join("references").exists());

        let mut reloaded = vec![
            ImageAttachment {
                bytes: Vec::new(),
                ..first.clone()
            },
            ImageAttachment {
                file: "missing.png".into(),
                bytes: Vec::new(),
                ..first.clone()
            },
            ImageAttachment {
                file: "../escape.png".into(),
                bytes: Vec::new(),
                ..first.clone()
            },
        ];
        load_attachment_bytes(project.path(), &mut reloaded);
        assert_eq!(reloaded[0].bytes, staged.data.bytes);
        assert!(reloaded[1].bytes.is_empty());
        assert!(reloaded[2].bytes.is_empty());
    }

    #[test]
    fn the_reference_library_lists_files_with_their_index_lines_and_keeps_images() {
        let project = tempfile::tempdir().unwrap();
        let library = ReferenceLibrary::of_project(project.path());
        assert!(library.entries().unwrap().is_empty());

        let references = project.path().join("references");
        std::fs::create_dir_all(&references).unwrap();
        std::fs::write(references.join("hand-placed.JPG"), b"x").unwrap();
        std::fs::write(references.join("notes.txt"), b"x").unwrap();

        let image = ImageData {
            media_type: "image/png".into(),
            bytes: png_bytes(4, 2, [0, 80, 160]),
        };
        let kept = library
            .keep(
                &image,
                "flange-top.jpg",
                "Top view of the flange,  bolt circle visible",
            )
            .unwrap();
        assert_eq!(kept, "flange-top.png", "the extension follows the format");
        let again = library.keep(&image, "flange-top", "Second copy").unwrap();
        assert_eq!(again, "flange-top-1.png");

        let entries = library.entries().unwrap();
        assert_eq!(
            entries,
            [
                ReferenceEntry {
                    file: "flange-top-1.png".into(),
                    description: Some("Second copy".into())
                },
                ReferenceEntry {
                    file: "flange-top.png".into(),
                    description: Some("Top view of the flange, bolt circle visible".into()),
                },
                ReferenceEntry {
                    file: "hand-placed.JPG".into(),
                    description: None
                },
            ]
        );
        assert_eq!(library.read("flange-top.png").unwrap(), image);
        assert!(library.read("../flange-top.png").is_err());
        assert!(library.read("notes.txt").is_err());
        assert!(library.read("hand-placed.JPG").is_err(), "not a real image");

        library
            .describe("hand-placed.JPG", "A photo the user dropped in")
            .unwrap();
        library
            .describe("flange-top.png", "Top view, revised")
            .unwrap();
        assert!(library.describe("nowhere.png", "x").is_err());
        assert!(library.describe("flange-top.png", "   ").is_err());
        let index = std::fs::read_to_string(references.join("INDEX.md")).unwrap();
        assert!(index.starts_with("# Reference images"));
        assert_eq!(
            parse_index(&index),
            [
                (
                    "flange-top.png".to_string(),
                    "Top view, revised".to_string()
                ),
                ("flange-top-1.png".to_string(), "Second copy".to_string()),
                (
                    "hand-placed.JPG".to_string(),
                    "A photo the user dropped in".to_string()
                ),
            ]
        );
    }
}
