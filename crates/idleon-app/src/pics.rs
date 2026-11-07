//! The game's pictures, from the ones the core carries. A handle is made once per picture and
//! kept: iced decodes a picture once per handle, so a new handle every frame would decode it
//! again every frame.

use std::cell::RefCell;
use std::collections::HashMap;

use iced::ContentFit;
use iced::widget::image::{FilterMethod, Handle, Image};

/// A picture's handle with its width and height, or `None` when it didn't read.
type Kept = Option<(Handle, u32, u32)>;

thread_local! {
    static HANDLES: RefCell<HashMap<&'static str, Kept>> = RefCell::new(HashMap::new());
}

/// A picture's width and height from its PNG header, without decoding it.
fn size(png: &[u8]) -> Option<(u32, u32)> {
    let w = png.get(16..20)?;
    let h = png.get(20..24)?;
    Some((
        u32::from_be_bytes(w.try_into().ok()?),
        u32::from_be_bytes(h.try_into().ok()?),
    ))
}

fn handle(id: &str) -> Option<(Handle, u32, u32)> {
    let (key, bytes) = idleon_core::pictures::entry(id)?;
    HANDLES.with(|h| {
        h.borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let (w, hgt) = size(bytes)?;
                // The cavern villagers' wide plates: their left square, the portrait, cut once here.
                if idleon_core::view::left_square(key) && w > hgt {
                    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                        .ok()?
                        .crop_imm(0, 0, hgt, hgt)
                        .to_rgba8();
                    return Some((Handle::from_rgba(hgt, hgt, img.into_raw()), hgt, hgt));
                }
                Some((Handle::from_bytes(bytes), w, hgt))
            })
            .clone()
    })
}

/// The picture at `px` square, pixel art kept sharp. `None` when the core has no such picture.
pub fn pic(id: &str, px: f32) -> Option<Image<Handle>> {
    let (h, _, _) = handle(id)?;
    Some(
        Image::new(h)
            .width(px)
            .height(px)
            .filter_method(FilterMethod::Nearest)
            .content_fit(ContentFit::Contain),
    )
}

/// Any image's bytes (the server's icon), as a handle made once.
pub fn bytes(b: Vec<u8>) -> Handle {
    Handle::from_bytes(b)
}
