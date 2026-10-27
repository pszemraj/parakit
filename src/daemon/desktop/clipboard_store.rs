//! Clipboard backend trait, its arboard adapter, and restorable payload snapshots.

#[cfg(not(target_os = "linux"))]
use super::clipboard_guard;
use super::ClipboardRestoreError;
use anyhow::{Context, Result};
use arboard::{Clipboard, ImageData};
#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
#[cfg(target_os = "macos")]
use objc2_foundation::NSString;
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use windows::{
    core::w,
    Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW},
};

/// System clipboard and the native resources used to observe its ownership.
pub(crate) struct SystemClipboard {
    clipboard: Clipboard,
    #[cfg(target_os = "linux")]
    observation: Option<(x11rb::rust_connection::RustConnection, u32)>,
}

impl SystemClipboard {
    /// Open the clipboard; Linux ownership observation initializes on first use.
    ///
    /// # Returns
    /// An owned clipboard backend.
    ///
    /// # Errors
    /// Reports a failure to open the system clipboard.
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            clipboard: Clipboard::new().context("could not open system clipboard")?,
            #[cfg(target_os = "linux")]
            observation: None,
        })
    }
}

/// Minimal clipboard operations used by insertion and smoke-test paths.
pub(crate) trait ClipboardStore {
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
fn restore_html_clipboard(
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

impl ClipboardStore for SystemClipboard {
    fn change_stamp(&mut self) -> Result<u64> {
        #[cfg(target_os = "linux")]
        {
            use x11rb::protocol::xproto::ConnectionExt;
            if self.observation.is_none() {
                let (connection, _) =
                    x11rb::connect(None).context("open clipboard observation connection")?;
                let selection = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
                self.observation = Some((connection, selection));
            }
            let (connection, selection) = self
                .observation
                .as_ref()
                .expect("clipboard observation was just initialized");
            let result = connection
                .get_selection_owner(*selection)
                .map_err(anyhow::Error::new)
                .and_then(|request| request.reply().map_err(anyhow::Error::new))
                .map(|reply| u64::from(reply.owner));
            if result.is_err() {
                self.observation = None;
            }
            result
        }
        #[cfg(not(target_os = "linux"))]
        clipboard_guard::platform_change_stamp()
    }

    fn get_text(&mut self) -> Result<String> {
        self.clipboard
            .get_text()
            .context("could not read system clipboard")
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        self.clipboard
            .set_text(text)
            .context("could not write system clipboard")
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
        self.clipboard
            .get()
            .html()
            .context("could not read HTML clipboard contents")
    }

    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()> {
        restore_html_clipboard(&mut self.clipboard, html, alt_text)
    }

    fn get_file_list(&mut self) -> Result<Vec<PathBuf>> {
        let files = self
            .clipboard
            .get()
            .file_list()
            .context("could not read file-list clipboard contents")?;
        #[cfg(target_os = "linux")]
        let files = linux_file_list_paths(files);
        Ok(files)
    }

    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()> {
        self.clipboard
            .set()
            .file_list(files)
            .context("could not write file-list clipboard contents")
    }

    fn get_image(&mut self) -> Result<ImageData<'static>> {
        self.clipboard
            .get_image()
            .context("could not read image clipboard contents")
    }

    fn set_image(&mut self, image: ImageData<'static>) -> Result<()> {
        self.clipboard
            .set_image(image)
            .context("could not write image clipboard contents")
    }

    fn clear(&mut self) -> Result<()> {
        self.clipboard
            .clear()
            .context("could not clear system clipboard")
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
#[derive(Clone)]
pub(crate) enum ClipboardSnapshot {
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
    pub(crate) fn capture<C: ClipboardStore>(clipboard: &mut C) -> Result<Self> {
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
                        Ok(image) => return Ok(Self::Image(image)),
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
            Ok(image) => return Ok(Self::Image(image)),
            Err(error) if clipboard_content_unavailable(&error) => {}
            Err(error) => return Err(error.context("could not snapshot image clipboard")),
        }

        match clipboard.get_text() {
            Ok(text) => Ok(Self::Text(text)),
            Err(error) if clipboard_content_unavailable(&error) => Ok(Self::Unsupported),
            Err(error) => Err(error.context("could not snapshot text clipboard")),
        }
    }

    /// Restore the saved payload without consuming it, so cleanup can retry.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard backend to update.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the saved payload was written or unsupported data cleared.
    ///
    /// # Errors
    ///
    /// Returns an error when the clipboard cannot accept the saved payload.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn restore<C: ClipboardStore>(&self, clipboard: &mut C) -> Result<()> {
        self.clone().restore_owned(clipboard)
    }

    /// Restore a consumed snapshot by moving its payload into the backend.
    ///
    /// # Arguments
    /// * `clipboard` - Clipboard backend receiving the saved payload.
    ///
    /// # Returns
    /// Success when the payload was restored or unsupported data cleared.
    ///
    /// # Errors
    /// Returns [`ClipboardRestoreError`] if the write or clearing fallback fails.
    pub(super) fn restore_owned<C: ClipboardStore>(self, clipboard: &mut C) -> Result<()> {
        let result = match self {
            Self::Text(text) => clipboard.set_text(text),
            Self::Html { html, alt_text } => clipboard.set_html(html, alt_text),
            Self::FileList(files) => clipboard.set_file_list(&files),
            Self::Image(image) => clipboard.set_image(image),
            Self::Unsupported => clipboard
                .clear()
                .or_else(|_| clipboard.set_text(String::new()))
                .context("previous clipboard format unsupported and staged transcript could not be cleared"),
        };
        result.map_err(|error| ClipboardRestoreError(error).into())
    }

    /// Compare every supported representation and its complete payload.
    ///
    /// # Arguments
    ///
    /// * `other` - Independently captured clipboard payload to compare.
    ///
    /// # Returns
    ///
    /// Whether the payload kinds and their contents match exactly.
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub(crate) fn same_payload(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Text(expected), Self::Text(actual)) => expected == actual,
            (
                Self::Html {
                    html: expected_html,
                    alt_text: expected_alt,
                },
                Self::Html {
                    html: actual_html,
                    alt_text: actual_alt,
                },
            ) => expected_html == actual_html && expected_alt == actual_alt,
            (Self::FileList(expected), Self::FileList(actual)) => expected == actual,
            (Self::Image(expected), Self::Image(actual)) => {
                expected.width == actual.width
                    && expected.height == actual.height
                    && expected.bytes == actual.bytes
            }
            (Self::Unsupported, Self::Unsupported) => true,
            _ => false,
        }
    }

    /// Check that this payload is plain text equal to the staged sentinel.
    ///
    /// # Arguments
    ///
    /// * `expected` - Sentinel text owned by the diagnostic.
    ///
    /// # Returns
    ///
    /// Whether this is plain text containing exactly the sentinel.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn is_text(&self, expected: &str) -> bool {
        matches!(self, Self::Text(actual) if actual == expected)
    }

    /// Name the captured payload kind in a clipboard diagnostic.
    ///
    /// # Returns
    ///
    /// A readable label for the supported kind or unsupported clipboard.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Html { .. } => "HTML",
            Self::FileList(_) => "file list",
            Self::Image(_) => "image",
            Self::Unsupported => "empty or unsupported",
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use x11rb::connection::Connection as _;

    #[test]
    #[ignore = "requires an X11 display; does not change clipboard contents"]
    fn ownership_reads_reuse_the_connection_and_atom() {
        let mut clipboard = SystemClipboard::new().unwrap();
        assert!(clipboard.observation.is_none());
        clipboard.change_stamp().unwrap();
        let (connection, atom) = clipboard.observation.as_ref().unwrap();
        let resource_base = connection.setup().resource_id_base;
        let atom = *atom;
        for _ in 0..3 {
            clipboard.change_stamp().unwrap();
            let (connection, cached_atom) = clipboard.observation.as_ref().unwrap();
            assert_eq!(connection.setup().resource_id_base, resource_base);
            assert_eq!(*cached_atom, atom);
        }
    }
}
