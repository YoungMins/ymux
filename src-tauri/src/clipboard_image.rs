//! Reading a pasted image off the **OS** clipboard, in Rust.
//!
//! Why not the webview: `TerminalPane` used to do this with
//! `navigator.clipboard.read()` → `blob.arrayBuffer()` → a JSON number array
//! over IPC → `paste_images::save`. On Windows that produced 0-byte PNGs —
//! the backend wrote exactly what it was handed, so the empty array came from
//! WebView2's async clipboard image read. Going straight to the OS clipboard
//! removes the webview, the blob, and the number-array IPC hop from the path
//! at once.
//!
//! Only the OS half lives here. Encoding the pixels as PNG is
//! [`crate::paste_images::encode_png_rgba`], which is `std`-only and therefore
//! unit-tested on Linux CI; this module is `desktop`-gated because `arboard`
//! pulls X11/Wayland deps on Linux.

use arboard::{Clipboard, Error, ImageData};

/// How long to wait before the single retry in [`read_clipboard_png`]. Short
/// enough to be invisible in a keystroke-driven paste, long enough for a
/// clipboard rendition that is still being written to land — measured at well
/// under 50 ms (see the round-trip test below).
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(40);

/// The PNG bytes of whatever image is on the system clipboard, or `Ok(None)`
/// if there isn't one.
///
/// **A missing image is not an error.** The caller's fallback for `Ok(None)`
/// is the ordinary text paste, so mapping every `arboard` failure through as
/// `Err` would break *all* pasting, not just image pasting. Everything up to
/// and including the clipboard read therefore degrades to `Ok(None)` (logged);
/// only a failure with pixels already in hand — a degenerate image, or a PNG
/// encode error — comes back as `Err` for the user to see.
///
/// A non-`ContentNotAvailable` failure is retried once. On Windows `arboard`
/// prefers the registered `PNG` clipboard format whenever it is *advertised*
/// and gives up (it does not fall back to `CF_DIBV5`) if reading it then
/// fails — which is exactly what happens while a rendition is still being
/// written: the round-trip test below fails on its first read every single
/// time and succeeds on every read after. `ContentNotAvailable` — the ordinary
/// "this clipboard holds text" answer — is *not* retried, so a plain text
/// paste never pays the delay.
pub fn read_clipboard_png() -> std::io::Result<Option<Vec<u8>>> {
    let image = match read_image() {
        Ok(Some(image)) => image,
        Ok(None) => return Ok(None),
        Err(first) => {
            tracing::debug!(error = %first, "clipboard image read failed, retrying once");
            std::thread::sleep(RETRY_DELAY);
            match read_image() {
                Ok(Some(image)) => image,
                Ok(None) => return Ok(None),
                Err(e) => {
                    tracing::warn!(error = %e, "giving up on reading the clipboard image");
                    return Ok(None);
                }
            }
        }
    };
    let width = u32::try_from(image.width).unwrap_or(0);
    let height = u32::try_from(image.height).unwrap_or(0);
    crate::paste_images::encode_png_rgba(width, height, &image.bytes).map(Some)
}

/// One attempt at the OS clipboard. `Ok(None)` is the definitive "there is no
/// image here" answer; `Err` is a failure that may be worth retrying.
fn read_image() -> Result<Option<ImageData<'static>>, Error> {
    match Clipboard::new()?.get_image() {
        Ok(image) => Ok(Some(image)),
        Err(Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// There is exactly one system clipboard, so the tests below cannot run
    /// concurrently: without this, whichever test sets its content second wins
    /// and the other one fails. `cargo test` runs test fns on parallel
    /// threads, and `--test-threads=1` is not something a reader of these
    /// tests should have to know about.
    static CLIPBOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Serialize on [`CLIPBOARD`], ignoring poisoning — a panic in one of
    /// these tests should show as that test's failure, not as a confusing
    /// cascade in the other.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// End-to-end against the **real** system clipboard: put a known image on
    /// it, read it back through the production path, and check the pixels
    /// survive.
    ///
    /// `#[ignore]`d on purpose. It needs an interactive desktop session (so it
    /// would fail on CI), and it *overwrites whatever the user has copied*.
    /// Run it deliberately:
    ///
    /// ```text
    /// cargo test -p ymux --lib -- --ignored clipboard_image
    /// ```
    ///
    /// It also covers the retry in [`read_clipboard_png`]: on Windows the read
    /// immediately following `set_image` fails every time, so without the
    /// retry this test would fail every time too.
    #[test]
    #[ignore = "needs a desktop session and clobbers the user's clipboard"]
    fn round_trips_a_real_clipboard_image() {
        let _guard = lock();
        #[rustfmt::skip]
        let rgba: [u8; 16] = [
            255, 0, 0, 255,    0, 255, 0, 255,
            0, 0, 255, 255,    255, 255, 255, 255,
        ];
        let mut clipboard = Clipboard::new().expect("open clipboard");
        clipboard
            .set_image(ImageData {
                width: 2,
                height: 2,
                bytes: std::borrow::Cow::Borrowed(&rgba),
            })
            .expect("set_image");

        let png_bytes = read_clipboard_png()
            .expect("read_clipboard_png")
            .expect("an image was just placed on the clipboard");
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");

        let mut reader = png::Decoder::new(std::io::Cursor::new(&png_bytes))
            .read_info()
            .expect("decode header");
        let info = reader.info().clone();
        assert_eq!((info.width, info.height), (2, 2));
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut buf).expect("decode pixels");
        assert_eq!(
            &buf[..frame.buffer_size()],
            &rgba[..],
            "clipboard round trip must preserve the pixels, alpha included"
        );
    }

    /// Text on the clipboard must be the cheap, definitive "no image" answer —
    /// `Ok(None)`, without the retry — or every ordinary text paste would pay
    /// the retry delay before the frontend reached its text fallback.
    #[test]
    #[ignore = "needs a desktop session and clobbers the user's clipboard"]
    fn text_on_the_clipboard_reports_no_image() {
        let _guard = lock();
        let mut clipboard = Clipboard::new().expect("open clipboard");
        clipboard.set_text("ymux clipboard test").expect("set_text");
        let started = std::time::Instant::now();
        let result = read_clipboard_png().expect("must not be an error");
        assert!(result.is_none(), "text is not an image");
        assert!(
            started.elapsed() < RETRY_DELAY,
            "the no-image answer must not go through the retry path"
        );
    }
}
