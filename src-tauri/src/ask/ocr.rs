//! Words on a picture, read by Windows' own OCR (Windows.Media.Ocr), so
//! nothing leaves the computer: what each mark on a grab or snap encloses,
//! for the prompt ("box 2 round \"queue full, dropping 2\"").

use std::path::Path;

use super::timeline::{MarkKind, What};

/// A word on a picture, its box in percent of the picture's size.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Word {
    fn middle(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
}

/// The most words a mark is said to enclose.
const MAX_WORDS: usize = 16;
/// How far outside a mark (percent) a word's middle may be and still count:
/// a circle drawn tight, an underline just below.
const SLACK: f32 = 1.5;
/// How near a pin's point (percent) the word it points at must be.
const PIN_REACH: f32 = 4.0;

/// The words on the PNG at `path`, in reading order. Empty when Windows has
/// no OCR language installed or the picture can't be read.
pub fn read_png(path: &Path) -> Vec<Word> {
    let Some((width, height, bgra)) = decode(path) else {
        return Vec::new();
    };
    match recognize(&bgra, width, height) {
        Ok(words) => words,
        Err(e) => {
            log::debug!("Ask: couldn't read the words on {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// What mark `what` encloses, from the words on its picture: those inside a
/// box, circle, highlight or stroke, or the one a pin points at. Arrows
/// point between marks or places, and text marks say their own words.
pub fn enclosed(words: &[Word], what: &What) -> Option<String> {
    let What::Mark {
        n,
        kind,
        region: Some(r),
        ..
    } = what
    else {
        return None;
    };
    // A mark's own number is drawn on the picture; it isn't a word there.
    let own = n.to_string();
    let words = words.iter().filter(|w| w.text != own);
    let picked: Vec<&str> = match kind {
        MarkKind::Arrow | MarkKind::Text => return None,
        MarkKind::Pin => {
            let tip = ((f32::from(r.x0) + f32::from(r.x1)) / 2.0, f32::from(r.y1));
            let distance = |w: &Word| {
                let dx = (w.x0 - tip.0).max(tip.0 - w.x1).max(0.0);
                let dy = (w.y0 - tip.1).max(tip.1 - w.y1).max(0.0);
                dx.hypot(dy)
            };
            words
                .filter(|w| distance(w) <= PIN_REACH)
                .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                .map(|w| vec![w.text.as_str()])
                .unwrap_or_default()
        }
        MarkKind::Box | MarkKind::Circle | MarkKind::Highlight | MarkKind::Pen => words
            .filter(|w| {
                let (x, y) = w.middle();
                x >= f32::from(r.x0) - SLACK
                    && x <= f32::from(r.x1) + SLACK
                    && y >= f32::from(r.y0) - SLACK
                    && y <= f32::from(r.y1) + SLACK
            })
            .map(|w| w.text.as_str())
            .take(MAX_WORDS)
            .collect(),
    };
    (!picked.is_empty()).then(|| picked.join(" "))
}

/// A PNG's pixels as BGRA, for OCR.
fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..info.buffer_size()];
    let bgra: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], 255])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, bgra))
}

/// Every `step`th pixel each way: OCR takes pictures only up to a size.
fn thin(bgra: &[u8], width: u32, height: u32, step: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width / step, height / step);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let at = (((y * step) * width + x * step) * 4) as usize;
            out.extend_from_slice(&bgra[at..at + 4]);
        }
    }
    (w, h, out)
}

#[cfg(windows)]
fn recognize(bgra: &[u8], width: u32, height: u32) -> windows::core::Result<Vec<Word>> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    // Fails when no OCR language is installed for the user's languages.
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let most = OcrEngine::MaxImageDimension()?.max(1);
    let step = width.max(height).div_ceil(most).max(1);
    let thinned;
    let (width, height, bgra) = if step > 1 {
        thinned = thin(bgra, width, height, step);
        (thinned.0, thinned.1, thinned.2.as_slice())
    } else {
        (width, height, bgra)
    };
    let writer = DataWriter::new()?;
    writer.WriteBytes(bgra)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer()?,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )?;
    let result = engine.RecognizeAsync(&bitmap)?.get()?;
    let (w, h) = (width as f32, height as f32);
    let mut words = Vec::new();
    for line in result.Lines()? {
        for word in line.Words()? {
            let r = word.BoundingRect()?;
            words.push(Word {
                text: word.Text()?.to_string_lossy(),
                x0: r.X / w * 100.0,
                x1: (r.X + r.Width) / w * 100.0,
                y0: r.Y / h * 100.0,
                y1: (r.Y + r.Height) / h * 100.0,
            });
        }
    }
    Ok(words)
}

#[cfg(not(windows))]
fn recognize(_bgra: &[u8], _width: u32, _height: u32) -> Result<Vec<Word>, String> {
    Err("OCR is only on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::timeline::Region;

    fn word(text: &str, x0: f32, x1: f32, y0: f32, y1: f32) -> Word {
        Word {
            text: text.into(),
            x0,
            x1,
            y0,
            y1,
        }
    }

    fn mark(n: u32, kind: MarkKind, x0: u8, x1: u8, y0: u8, y1: u8) -> What {
        What::Mark {
            n,
            kind,
            on: "Grab 1".into(),
            region: Some(Region { x0, x1, y0, y1 }),
            joins: None,
            encloses: None,
            label: None,
        }
    }

    fn page() -> Vec<Word> {
        vec![
            word("queue", 10.0, 18.0, 10.0, 14.0),
            word("full,", 19.0, 25.0, 10.0, 14.0),
            word("dropping", 26.0, 38.0, 10.0, 14.0),
            word("2", 39.0, 41.0, 10.0, 14.0),
            word("stream", 10.0, 19.0, 40.0, 44.0),
            word("done", 20.0, 27.0, 40.0, 44.0),
        ]
    }

    #[test]
    fn a_box_encloses_the_words_inside_it_in_order() {
        let boxed = mark(3, MarkKind::Box, 8, 43, 8, 16);
        assert_eq!(
            enclosed(&page(), &boxed).as_deref(),
            Some("queue full, dropping 2")
        );
    }

    #[test]
    fn a_marks_own_number_is_not_one_of_its_words() {
        let boxed = mark(2, MarkKind::Box, 8, 43, 8, 16);
        assert_eq!(
            enclosed(&page(), &boxed).as_deref(),
            Some("queue full, dropping")
        );
    }

    #[test]
    fn a_pin_points_at_the_word_at_its_tip() {
        // The pin stands above its point, the middle of its foot.
        let pin = mark(1, MarkKind::Pin, 20, 26, 32, 41);
        assert_eq!(enclosed(&page(), &pin).as_deref(), Some("done"));
        let far = mark(1, MarkKind::Pin, 70, 76, 60, 70);
        assert_eq!(enclosed(&page(), &far), None);
    }

    #[test]
    fn arrows_text_and_empty_places_enclose_nothing() {
        let arrow = mark(1, MarkKind::Arrow, 0, 100, 0, 100);
        let text = mark(1, MarkKind::Text, 0, 100, 0, 100);
        let empty = mark(1, MarkKind::Box, 60, 90, 60, 90);
        for what in [arrow, text, empty] {
            assert_eq!(enclosed(&page(), &what), None);
        }
    }

    #[test]
    fn thinning_keeps_every_nth_pixel() {
        let bgra: Vec<u8> = (0..4u8 * 2).flat_map(|i| [i, i, i, 255]).collect();
        let (w, h, out) = thin(&bgra, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, [0, 0, 0, 255, 2, 2, 2, 255]);
    }

    /// Draws `text` black on white into a bitmap in memory, with GDI: no
    /// screen is read.
    #[cfg(windows)]
    fn drawn(text: &str, width: i32, height: i32) -> Vec<u8> {
        use windows::core::w;
        use windows::Win32::Foundation::{COLORREF, RECT};
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, FillRect,
            GdiFlush, GetStockObject, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
            DIB_RGB_COLORS, FW_NORMAL, HBRUSH, OUT_DEFAULT_PRECIS, TRANSPARENT, WHITE_BRUSH,
        };

        // SAFETY: every GDI object made here is released before returning;
        // the bits are read while the section is alive and sized as made.
        unsafe {
            let memory = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let previous = SelectObject(memory, bitmap.into());
            let all = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            FillRect(memory, &all, HBRUSH(GetStockObject(WHITE_BRUSH).0));
            let font = CreateFontW(
                40,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let previous_font = SelectObject(memory, font.into());
            SetTextColor(memory, COLORREF(0));
            SetBkMode(memory, TRANSPARENT);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let _ = TextOutW(memory, 24, 30, &wide);
            let _ = GdiFlush();
            let pixels =
                std::slice::from_raw_parts(bits.cast::<u8>(), (width * height * 4) as usize)
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect();
            SelectObject(memory, previous_font);
            SelectObject(memory, previous);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            pixels
        }
    }

    #[cfg(windows)]
    #[test]
    fn reads_words_drawn_in_memory() {
        let (width, height) = (640, 110);
        let pixels = drawn("queue full dropping", width, height);
        let words = match recognize(&pixels, width as u32, height as u32) {
            Ok(words) => words,
            // No OCR language installed here: nothing to check.
            Err(_) => return,
        };
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(text, ["queue", "full", "dropping"]);
        assert!(words[0].x0 < words[1].x0 && words[1].x0 < words[2].x0);
        assert!(words.iter().all(|w| w.y0 > 10.0 && w.y1 < 90.0));
    }
}
