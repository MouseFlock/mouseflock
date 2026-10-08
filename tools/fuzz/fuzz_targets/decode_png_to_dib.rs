//! A PNG from another machine, decoded as the session agent would before
//! putting it on the clipboard: errors are fine, panics and runaway
//! allocations are not. Whatever decodes is a bottom-up 32 bpp bitmap whose
//! size matches its header, and converting it back to PNG gives the same
//! dimensions. Every input is also tried as a copied bitmap.

#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use win32_clipboard::image::{check_png, dib_to_png, png_to_dibv5};

fuzz_target!(|data: &[u8]| {
    let header = check_png(data);
    if let Ok(dib) = png_to_dibv5(data) {
        let (w, h) = header.expect("a decoded PNG passed the header check");
        assert_eq!(dib.len(), 124 + (w as usize) * (h as usize) * 4);
        let again = dib_to_png(&dib).expect("our own bitmap converts back");
        assert_eq!(check_png(&again), Ok((w, h)));
    }
    let _ = dib_to_png(data);
});
