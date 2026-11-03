//! Pixels crossing the boundary: frames on their way into a document, and
//! rendered images on their way out.

use gifkino_core::Frame;
use image::RgbaImage;
use wasm_bindgen::Clamped;
use wasm_bindgen::prelude::*;

use crate::error;

/// A still image spliced into an empty document has no neighbour to take a
/// delay from. Same default as the desktop app's still import.
const STILL_DELAY_CS: u16 = 10;

/// Decoded frames waiting to become a document (`Editor.open`) or to be
/// spliced into one (`Editor.splice`). Holding them in a handle lets the UI
/// ask the fit question between the decode and the splice without copying
/// pixels back out to JavaScript.
#[wasm_bindgen]
#[derive(Default)]
pub struct Frames {
    pub(crate) frames: Vec<Frame>,
}

#[wasm_bindgen]
impl Frames {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Frames {
        Frames::default()
    }

    /// Every frame of a GIF, full-canvas, disposal applied. `progress` gets
    /// the count decoded so far; returning `false` stops the decode there.
    #[wasm_bindgen(js_name = fromGif)]
    pub fn from_gif(
        bytes: &[u8],
        #[wasm_bindgen(unchecked_param_type = "((decoded: number) => boolean) | null")]
        progress: Option<js_sys::Function>,
    ) -> Result<Frames, JsError> {
        let mut report = |done: usize, _: Option<usize>| {
            progress.as_ref().is_none_or(|callback| {
                callback
                    .call1(&JsValue::NULL, &JsValue::from(done as u32))
                    .is_ok_and(|keep_going| keep_going.as_bool() != Some(false))
            })
        };
        let frames = gifkino_core::gif::decode(std::io::Cursor::new(bytes), &mut report)
            .map_err(|e| error(format!("{e:#}")))?;
        Ok(Frames { frames })
    }

    /// One still image (PNG, JPEG, BMP, TIFF, TGA, ICO, QOI, PNM) as one
    /// frame.
    #[wasm_bindgen(js_name = fromImage)]
    pub fn from_image(bytes: &[u8]) -> Result<Frames, JsError> {
        let pixels = image::load_from_memory(bytes)
            .map_err(|e| error(format!("decoding image: {e}")))?
            .to_rgba8();
        Ok(Frames {
            frames: vec![Frame::new(pixels, STILL_DELAY_CS)],
        })
    }

    /// Append one frame of raw RGBA, `width * height * 4` bytes — a video
    /// frame the browser decoded, say. Every frame in a batch should share a
    /// size; a splice fits them to the canvas by the first one's.
    pub fn push(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        delay_cs: u16,
    ) -> Result<(), JsError> {
        let pixels = RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
            error(format!(
                "{} bytes is not a {width}x{height} RGBA frame",
                rgba.len()
            ))
        })?;
        self.frames.push(Frame::new(pixels, delay_cs));
        Ok(())
    }

    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.frames.len()
    }

    /// The first frame's width, 0 with none.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.frames.first().map_or(0, |f| f.pixels.width())
    }

    /// The first frame's height, 0 with none.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.frames.first().map_or(0, |f| f.pixels.height())
    }
}

/// RGBA pixels: `data` is `width * height * 4` bytes, ready for
/// `new ImageData(image.data, image.width, image.height)`.
#[wasm_bindgen]
pub struct Image {
    pixels: RgbaImage,
}

impl From<RgbaImage> for Image {
    fn from(pixels: RgbaImage) -> Self {
        Image { pixels }
    }
}

#[wasm_bindgen]
impl Image {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// A copy of the pixels; read it once and keep it.
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Clamped<Vec<u8>> {
        Clamped(self.pixels.as_raw().clone())
    }
}
