//! Reading a pasted image from the system clipboard.
//!
//! The windowing layer handles text pastes itself and reports nothing
//! for a clipboard that holds no text, so an image paste reaches the chat
//! as a bare shortcut. This module answers it: the clipboard is asked for
//! an image, and the pixels are encoded as a PNG for the store. Screenshot
//! tools and browsers offer copied pictures as `image/png`, which is what
//! the clipboard library asks for on Linux.

use cadmark_ui::chat::StagedImage;

use crate::reference_images::stage_rgba;

/// The image on the clipboard, or `None` when it holds none. A clipboard
/// that cannot be reached at all is an error.
pub fn read_image() -> Result<Option<StagedImage>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    match clipboard.get_image() {
        Ok(image) => stage_rgba(
            "Pasted image".to_string(),
            image.width,
            image.height,
            &image.bytes,
        )
        .map(Some),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
