//! Clipboard backend trait, its arboard adapter, and restorable payload snapshots.

use super::clipboard_guard;
use super::{ClipboardPolicy, CLIPBOARD_RESTORE_ERROR};
use anyhow::{Context, Result};
use arboard::{Clipboard, ImageData};
#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
#[cfg(target_os = "macos")]
use objc2_foundation::NSString;
use std::{borrow::Cow, path::PathBuf};
#[cfg(target_os = "windows")]
use windows::{
    core::w,
    Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW},
};

/// Minimal clipboard operations used by insertion and smoke-test paths.
pub(in crate::daemon::desktop) trait ClipboardStore {
    /// Observe the native clipboard generation, or the selection owner on X11.
    ///
    /// # Returns
    ///
    /// A stamp that changes when another clipboard write is observable.
    /// X11 owner stamps do not detect updates by the same owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the clipboard state cannot be observed.
    fn change_stamp(&mut self) -> Result<u64>;

    /// Return the current text clipboard contents.
    ///
    /// # Returns
    ///
    /// The current text clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard is unavailable or does not hold text.
    fn get_text(&mut self) -> Result<String>;
    /// Replace the current text clipboard contents.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the new text.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_text(&mut self, text: String) -> Result<()>;

    /// Return current HTML clipboard contents.
    ///
    /// # Returns
    ///
    /// The current HTML clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose HTML data.
    fn get_html(&mut self) -> Result<String>;

    /// Replace the clipboard with HTML and an optional plain-text alternative.
    ///
    /// # Arguments
    ///
    /// * `html` - HTML payload to restore.
    /// * `alt_text` - Optional plain-text alternative for targets that prefer text.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the HTML data.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()>;

    /// Return current file-list clipboard contents.
    ///
    /// # Returns
    ///
    /// The current list of copied file paths.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose file-list data.
    fn get_file_list(&mut self) -> Result<Vec<PathBuf>>;

    /// Replace the clipboard with a file-list payload.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the file list.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()>;

    /// Return current image clipboard contents.
    ///
    /// # Returns
    ///
    /// The current image clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose image data.
    fn get_image(&mut self) -> Result<ImageData<'static>>;

    /// Replace the clipboard with image data.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the image.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_image(&mut self, image: ImageData<'static>) -> Result<()>;

    /// Clear all current clipboard contents.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard was cleared.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be cleared.
    fn clear(&mut self) -> Result<()>;
}

/// Restore HTML and its optional plain-text alternative to the clipboard.
///
/// arboard's macOS HTML setter always surrounds the supplied HTML with a
/// synthetic document wrapper. That is useful when copying a fresh fragment,
/// but restoring an already-captured payload through it nests another wrapper
/// after every dictation. Write the two native pasteboard types directly on
/// macOS so the captured HTML is preserved verbatim.
///
/// # Arguments
///
/// * `clipboard` - Open clipboard handle used by non-macOS backends.
/// * `html` - Captured HTML payload to restore.
/// * `alt_text` - Optional plain-text representation of the same payload.
///
/// # Returns
///
/// `Ok(())` when every requested pasteboard representation was written.
///
/// # Errors
///
/// Returns an error if the platform clipboard rejects either representation.
pub(crate) fn restore_html_clipboard(
    clipboard: &mut Clipboard,
    html: String,
    alt_text: Option<String>,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = clipboard;
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: AppKit exports these immutable, process-lifetime
            // pasteboard type constants whenever the linked framework is
            // loaded.
            let (html_type, string_type) =
                unsafe { (NSPasteboardTypeHTML, NSPasteboardTypeString) };

            if !pasteboard.setString_forType(&NSString::from_str(&html), html_type) {
                return Err(anyhow::anyhow!(
                    "could not write native HTML clipboard contents"
                ));
            }
            if let Some(alt_text) = alt_text {
                if !pasteboard.setString_forType(&NSString::from_str(&alt_text), string_type) {
                    return Err(anyhow::anyhow!(
                        "could not write native plain-text clipboard alternative"
                    ));
                }
            }
            Ok(())
        })
    }

    #[cfg(not(target_os = "macos"))]
    clipboard
        .set()
        .html(html, alt_text)
        .context("could not write HTML clipboard contents")
}

impl ClipboardStore for Clipboard {
    fn change_stamp(&mut self) -> Result<u64> {
        clipboard_guard::platform_change_stamp()
    }

    fn get_text(&mut self) -> Result<String> {
        Clipboard::get_text(self).context("could not read system clipboard")
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        Clipboard::set_text(self, text).context("could not write system clipboard")
    }

    fn get_html(&mut self) -> Result<String> {
        #[cfg(target_os = "windows")]
        {
            // arboard reports an absent Windows HTML format as `Unknown`,
            // which would make ordinary text, image, and empty clipboards look
            // unreadable. Ask Windows whether that representation exists so
            // snapshot capture can distinguish absence from a real read error.
            let html_format = unsafe { RegisterClipboardFormatW(w!("HTML Format")) };
            if html_format == 0 {
                return Err(anyhow::anyhow!(
                    "could not register Windows HTML clipboard format"
                ));
            }
            if unsafe { IsClipboardFormatAvailable(html_format) }.is_err() {
                return Err(arboard::Error::ContentNotAvailable.into());
            }
        }
        self.get()
            .html()
            .context("could not read HTML clipboard contents")
    }

    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()> {
        restore_html_clipboard(self, html, alt_text)
    }

    fn get_file_list(&mut self) -> Result<Vec<PathBuf>> {
        let files = self
            .get()
            .file_list()
            .context("could not read file-list clipboard contents")?;
        #[cfg(target_os = "linux")]
        let files = linux_file_list_paths(files);
        Ok(files)
    }

    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()> {
        self.set()
            .file_list(files)
            .context("could not write file-list clipboard contents")
    }

    fn get_image(&mut self) -> Result<ImageData<'static>> {
        Clipboard::get_image(self).context("could not read image clipboard contents")
    }

    fn set_image(&mut self, image: ImageData<'static>) -> Result<()> {
        Clipboard::set_image(self, image).context("could not write image clipboard contents")
    }

    fn clear(&mut self) -> Result<()> {
        Clipboard::clear(self).context("could not clear system clipboard")
    }
}

/// Drop the CR that arboard retains from CRLF-separated file-list entries.
///
/// # Arguments
///
/// * `files` - File paths as decoded by arboard.
///
/// # Returns
///
/// The same paths, with a trailing CR removed only when the CR-suffixed path
/// does not exist but its stripped sibling does.
#[cfg(target_os = "linux")]
pub(super) fn linux_file_list_paths(files: Vec<PathBuf>) -> Vec<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // arboard 3.6.1 retains CR from CRLF separators, but decoding also turns
    // an encoded filename suffix %0D into CR. Preserve any existing directory
    // entry, including a dangling symlink; never substitute its valid sibling.
    // Use the stripped path only when that directory entry exists.
    files
        .into_iter()
        .map(|path| {
            let Some(bytes) = path.as_os_str().as_bytes().strip_suffix(b"\r") else {
                return path;
            };
            if path.symlink_metadata().is_ok() {
                return path;
            }
            let stripped = PathBuf::from(OsStr::from_bytes(bytes));
            if stripped.symlink_metadata().is_ok() {
                stripped
            } else {
                path
            }
        })
        .collect()
}

/// Best-effort snapshot of supported clipboard payloads before staging text.
pub(in crate::daemon::desktop) enum ClipboardSnapshot {
    Text(String),
    Html {
        html: String,
        alt_text: Option<String>,
    },
    FileList(Vec<PathBuf>),
    Image(ImageData<'static>),
    Unsupported,
}

impl ClipboardSnapshot {
    /// Capture the current clipboard payload if it is one of the supported kinds.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard backend to inspect.
    ///
    /// # Returns
    ///
    /// A supported clipboard snapshot, or [`ClipboardSnapshot::Unsupported`]
    /// when the clipboard has no payload Parakit can restore.
    ///
    /// # Errors
    ///
    /// Returns an error when reading an available clipboard payload fails.
    pub(in crate::daemon::desktop) fn capture<C: ClipboardStore>(
        clipboard: &mut C,
    ) -> Result<Self> {
        match clipboard.get_file_list() {
            Ok(files) if !files.is_empty() => return Ok(Self::FileList(files)),
            // A text/uri-list without file:// entries parses as an empty file list.
            Ok(_) => {}
            Err(error) if clipboard_content_unavailable(&error) => {}
            Err(error) => return Err(error.context("could not snapshot file-list clipboard")),
        }

        match clipboard.get_html() {
            Ok(html) => {
                let alt_text = match clipboard.get_text() {
                    Ok(text) => Some(text),
                    Err(error) if clipboard_content_unavailable(&error) => None,
                    Err(error) => {
                        return Err(error.context("could not snapshot clipboard text alternative"));
                    }
                };
                // Browser image copies can expose transient HTML plus bitmap data.
                // Without a text alternative, the decoded image is usually the
                // restorable user-visible payload.
                if alt_text.as_deref().is_none_or(str::is_empty) {
                    match clipboard.get_image() {
                        Ok(image) => return Ok(Self::Image(owned_image(image))),
                        Err(error) if clipboard_content_unavailable(&error) => {}
                        Err(error) => {
                            return Err(error.context("could not snapshot image clipboard"));
                        }
                    }
                }
                return Ok(Self::Html { html, alt_text });
            }
            Err(error) if clipboard_content_unavailable(&error) => {}
            Err(error) => return Err(error.context("could not snapshot HTML clipboard")),
        }

        match clipboard.get_image() {
            Ok(image) => return Ok(Self::Image(owned_image(image))),
            Err(error) if clipboard_content_unavailable(&error) => {}
            Err(error) => return Err(error.context("could not snapshot image clipboard")),
        }

        match clipboard.get_text() {
            Ok(text) => Ok(Self::Text(text)),
            Err(error) if clipboard_content_unavailable(&error) => Ok(Self::Unsupported),
            Err(error) => Err(error.context("could not snapshot text clipboard")),
        }
    }
}

/// Report whether a clipboard read failed only because the format is not offered.
///
/// # Arguments
///
/// * `error` - Error returned by a clipboard read.
///
/// # Returns
///
/// `true` for arboard's content-not-available error and for the X11 owner
/// mismatch that means the same thing.
pub(super) fn clipboard_content_unavailable(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<arboard::Error>() {
        Some(arboard::Error::ContentNotAvailable) => true,
        // X11 owners such as xclip answer every requested format with their
        // own data type. arboard 3.6 reports that mismatch as an unknown
        // error, but the requested format is simply not offered.
        Some(arboard::Error::Unknown { description }) => {
            description == "incorrect type received from clipboard"
        }
        _ => false,
    }
}

/// Copy a borrowed clipboard image payload into an owned, `'static` one.
///
/// # Arguments
///
/// * `image` - Image data borrowed from a clipboard read.
///
/// # Returns
///
/// An equivalent `ImageData` that owns its pixel bytes.
pub(crate) fn owned_image(image: ImageData<'_>) -> ImageData<'static> {
    ImageData {
        width: image.width,
        height: image.height,
        bytes: Cow::Owned(image.bytes.into_owned()),
    }
}

/// Restore a previous clipboard snapshot unless the transcript should be retained.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to update.
/// * `previous` - Snapshot captured before staging transcript text.
/// * `clipboard_policy` - Policy deciding whether restoration should occur.
///
/// # Returns
///
/// `Ok(())` when restoration is skipped or the previous payload is restored.
///
/// # Errors
///
/// Returns an error if the previous supported payload cannot be written back
/// to the clipboard.
pub(in crate::daemon::desktop) fn restore_or_clear_clipboard<C: ClipboardStore>(
    clipboard: &mut C,
    previous: ClipboardSnapshot,
    clipboard_policy: ClipboardPolicy,
) -> Result<()> {
    if clipboard_policy == ClipboardPolicy::KeepTranscript {
        return Ok(());
    }
    match previous {
        ClipboardSnapshot::Text(previous) => clipboard
            .set_text(previous)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Html { html, alt_text } => clipboard
            .set_html(html, alt_text)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::FileList(files) => clipboard
            .set_file_list(&files)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Image(image) => clipboard
            .set_image(image)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Unsupported => clipboard
            .clear()
            .or_else(|_| clipboard.set_text(String::new()))
            .map_err(|err| {
                anyhow::anyhow!(
                    "{CLIPBOARD_RESTORE_ERROR}: previous clipboard format unsupported and staged transcript could not be cleared: {err:#}"
                )
            }),
    }
}
