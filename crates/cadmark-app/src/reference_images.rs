//! Project-owned photographs and drawings, independent of conversation history.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cadmark_bridge::backend::ImageData;

pub struct ReferenceImage {
    pub name: String,
    pub data: ImageData,
    pub thumbnail: egui::ColorImage,
}

#[derive(Default)]
pub struct ReferenceImages {
    pub images: Vec<ReferenceImage>,
    pub errors: Vec<String>,
}

impl ReferenceImages {
    pub fn load(project: &Path) -> Self {
        let mut result = Self::default();
        let entries = match std::fs::read_dir(project.join("references")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return result,
            Err(error) => {
                result
                    .errors
                    .push(format!("Could not read reference images: {error}"));
                return result;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) if image_format(&entry.path()).is_some() => paths.push(entry.path()),
                Ok(_) => {}
                Err(error) => result
                    .errors
                    .push(format!("Could not read a reference entry: {error}")),
            }
        }
        paths.sort();
        for path in paths {
            match read_image(&path) {
                Ok(image) => result.images.push(image),
                Err(error) => result.errors.push(error),
            }
        }
        result
    }

    pub fn inputs(&self) -> Vec<ImageData> {
        self.images.iter().map(|image| image.data.clone()).collect()
    }
}

fn image_format(path: &Path) -> Option<(image::ImageFormat, &'static str)> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some((image::ImageFormat::Png, "image/png")),
        "jpg" | "jpeg" => Some((image::ImageFormat::Jpeg, "image/jpeg")),
        _ => None,
    }
}

fn read_image(path: &Path) -> Result<ReferenceImage, String> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let read = || -> Result<ReferenceImage, String> {
        let (format, media_type) = image_format(path).ok_or("Choose a PNG or JPEG image")?;
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        // ImageReader's allocation limits apply before decoding a photograph.
        let decoded = image::ImageReader::with_format(Cursor::new(&bytes), format)
            .decode()
            .map_err(|error| error.to_string())?;
        let thumbnail = decoded.thumbnail(96, 96).to_rgba8();
        Ok(ReferenceImage {
            name: name.clone(),
            data: ImageData {
                media_type: media_type.into(),
                bytes,
            },
            thumbnail: egui::ColorImage::from_rgba_unmultiplied(
                [thumbnail.width() as usize, thumbnail.height() as usize],
                thumbnail.as_raw(),
            ),
        })
    };
    read().map_err(|error| format!("{name}: {error}"))
}

/// Copy validated bytes atomically without replacing an existing reference.
pub fn attach(project: &Path, source: &Path) -> Result<String, String> {
    let image = read_image(source)?;
    let directory = project.join("references");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(&image.data.bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let extension = source
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    for suffix in 0.. {
        let name = if suffix == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}-{suffix}.{extension}")
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

pub struct AttachmentResult {
    pub project: PathBuf,
    pub added: usize,
    pub errors: Vec<String>,
    pub references: ReferenceImages,
}

/// The picker and image decoding run outside the UI thread. Its destination
/// is captured when opened, so changing projects cannot redirect an attachment.
#[derive(Default)]
pub struct ReferenceImagePicker {
    receiver: Option<mpsc::Receiver<Option<AttachmentResult>>>,
}

impl ReferenceImagePicker {
    pub fn pending(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn open(
        &mut self,
        frame: &eframe::Frame,
        ctx: &egui::Context,
        project: PathBuf,
    ) -> Result<(), String> {
        if self.pending() {
            return Ok(());
        }
        let dialog = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title("Attach reference images to this project")
            .set_directory(&project)
            .add_filter(
                "PNG and JPEG images",
                &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"],
            );
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("cadmark-reference-picker".into())
            .spawn(move || {
                let result = dialog.pick_files().map(|files| {
                    let mut added = 0;
                    let mut errors = Vec::new();
                    for file in files {
                        match attach(&project, &file) {
                            Ok(_) => added += 1,
                            Err(error) => {
                                errors.push(format!("Could not attach {}: {error}", file.display()))
                            }
                        }
                    }
                    let references = ReferenceImages::load(&project);
                    AttachmentResult {
                        project,
                        added,
                        errors,
                        references,
                    }
                });
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .map_err(|error| format!("Could not open the image picker: {error}"))?;
        self.receiver = Some(receiver);
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<AttachmentResult>, String> {
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

    #[test]
    fn reference_copies_preserve_original_bytes_and_bounded_previews_after_sources_are_deleted() {
        let project = tempfile::tempdir().unwrap();
        let sources = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for name in ["drawing.PNG", "photo.JPEG"] {
            let source = sources.path().join(name);
            image::RgbImage::from_pixel(480, 240, image::Rgb([20, 80, 160]))
                .save(&source)
                .unwrap();
            originals.push(std::fs::read(&source).unwrap());
            attach(project.path(), &source).unwrap();
        }
        sources.close().unwrap();
        let loaded = ReferenceImages::load(project.path());
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.images.len(), 2);
        for (image, bytes) in loaded.images.iter().zip(originals) {
            assert_eq!(image.data.bytes, bytes);
            assert_eq!(image.thumbnail.size, [96, 48]);
        }
        assert_eq!(loaded.images[0].data.media_type, "image/png");
        assert_eq!(loaded.images[1].data.media_type, "image/jpeg");
        assert!(
            ReferenceImages::load(&project.path().join("another-project"))
                .images
                .is_empty()
        );
    }

    #[test]
    fn same_named_references_do_not_overwrite_and_invalid_images_leave_no_attachment() {
        let project = tempfile::tempdir().unwrap();
        let sources = tempfile::tempdir().unwrap();
        let source = sources.path().join("drawing.png");
        image::RgbImage::from_pixel(4, 2, image::Rgb([0, 80, 160]))
            .save(&source)
            .unwrap();
        assert_eq!(attach(project.path(), &source).unwrap(), "drawing.png");
        let original = std::fs::read(&source).unwrap();
        image::RgbImage::from_pixel(4, 2, image::Rgb([160, 80, 0]))
            .save(&source)
            .unwrap();
        assert_eq!(attach(project.path(), &source).unwrap(), "drawing-1.png");
        assert_eq!(
            std::fs::read(project.path().join("references/drawing.png")).unwrap(),
            original
        );
        std::fs::write(&source, b"not an image").unwrap();
        assert!(
            attach(project.path(), &source)
                .unwrap_err()
                .contains("drawing.png")
        );
        assert_eq!(
            std::fs::read_dir(project.path().join("references"))
                .unwrap()
                .count(),
            2
        );
        std::fs::write(project.path().join("references/broken.png"), b"broken").unwrap();
        let loaded = ReferenceImages::load(project.path());
        assert_eq!(loaded.images.len(), 2);
        assert_eq!(loaded.errors.len(), 1);
        assert!(loaded.errors[0].contains("broken.png"));
    }
}
