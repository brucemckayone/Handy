//! Words on a picture, read by Windows' own OCR (Windows.Media.Ocr), so
//! nothing leaves the computer: what each mark on a grab or snap encloses
//! ("box 2 round \"queue full, dropping 2\""), and what the mouse was on
//! at each "this" said ("pointing at \"Run tests\"").

use std::path::Path;

use super::timeline::{is_deictic, Event, MarkKind, Spoken, What};

/// The words on a picture, in reading order, and its size.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub words: Vec<Word>,
}

/// A word on a picture, its box in the picture's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
    /// Which line of text it is on.
    pub line: u32,
}

impl Word {
    fn middle(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    /// How far `(x, y)` is from the word's box; nothing when inside it.
    fn distance(&self, (x, y): (f32, f32)) -> f32 {
        let dx = (self.x0 - x).max(x - self.x1).max(0.0);
        let dy = (self.y0 - y).max(y - self.y1).max(0.0);
        dx.hypot(dy)
    }
}

impl Page {
    /// A point in percent of the picture, in its pixels.
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.width / 100.0, y * self.height / 100.0)
    }

    /// The words line by line, as text.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut line = None;
        for word in &self.words {
            match line {
                Some(n) if n == word.line => out.push(' '),
                Some(_) => out.push('\n'),
                None => {}
            }
            out.push_str(&word.text);
            line = Some(word.line);
        }
        out
    }

    /// `percent` of the picture's longer side, in pixels.
    fn reach(&self, percent: f32) -> f32 {
        self.width.max(self.height) * percent / 100.0
    }
}

/// The most words a mark is said to enclose.
const MAX_WORDS: usize = 16;
/// How far outside a mark (percent) a word's middle may be and still count:
/// a circle drawn tight, an underline just below.
const SLACK: f32 = 1.5;
/// How near a pin's point (percent of the longer side) the word it points
/// at must be.
const PIN_REACH: f32 = 4.0;
/// How near the mouse (percent of the longer side) a word must be for the
/// mouse to be on it.
const POINTER_REACH: f32 = 1.0;
/// The most words either side of the one under the mouse that run on with
/// it, as a button's or a link's whole text.
const RUN_ON: usize = 4;
/// How long before or after a "this" the mouse may have been seen.
const POINTER_GAP_MS: u64 = 450;

/// The words on the PNG at `path`. Empty when Windows has no OCR language
/// installed or the picture can't be read.
pub fn read_png(path: &Path) -> Page {
    let Some((width, height, bgra)) = decode(path) else {
        return Page::default();
    };
    match recognize(&bgra, width, height) {
        Ok(page) => page,
        Err(e) => {
            log::debug!("Ask: couldn't read the words on {}: {e}", path.display());
            Page::default()
        }
    }
}

/// What mark `what` encloses, from the words on its picture: those inside a
/// box, circle, highlight or stroke, or the one a pin points at. Arrows
/// point between marks or places, and text marks say their own words.
pub fn enclosed(page: &Page, what: &What) -> Option<String> {
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
    let words = page.words.iter().filter(|w| w.text != own);
    let picked: Vec<&str> = match kind {
        MarkKind::Arrow | MarkKind::Text => return None,
        MarkKind::Pin => {
            let tip = page.at((f32::from(r.x0) + f32::from(r.x1)) / 2.0, f32::from(r.y1));
            let reach = page.reach(PIN_REACH);
            words
                .filter(|w| w.distance(tip) <= reach)
                .min_by(|a, b| a.distance(tip).total_cmp(&b.distance(tip)))
                .map(|w| vec![w.text.as_str()])
                .unwrap_or_default()
        }
        MarkKind::Box | MarkKind::Circle | MarkKind::Highlight | MarkKind::Pen => {
            let (x0, y0) = page.at(f32::from(r.x0) - SLACK, f32::from(r.y0) - SLACK);
            let (x1, y1) = page.at(f32::from(r.x1) + SLACK, f32::from(r.y1) + SLACK);
            words
                .filter(|w| {
                    let (x, y) = w.middle();
                    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
                })
                .map(|w| w.text.as_str())
                .take(MAX_WORDS)
                .collect()
        }
    };
    (!picked.is_empty()).then(|| picked.join(" "))
}

/// The words the mouse is on at (`x`, `y`), percent of the picture: the
/// word there, or just by it, and those running on from it along its line,
/// so a button or link reads whole.
pub fn under(page: &Page, x: f32, y: f32) -> Option<String> {
    let point = page.at(x, y);
    let reach = page.reach(POINTER_REACH);
    let words = &page.words;
    let (at, _) = words
        .iter()
        .enumerate()
        .map(|(i, w)| (i, w.distance(point)))
        .filter(|&(_, d)| d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    // Words run on while the gap between them is no more than a space.
    let runs_on = |a: &Word, b: &Word| {
        let space = 0.6 * (a.y1 - a.y0).max(b.y1 - b.y0);
        a.line == b.line && b.x0 - a.x1 <= space
    };
    let mut first = at;
    while first > 0 && at - first < RUN_ON && runs_on(&words[first - 1], &words[first]) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < words.len() && last - at < RUN_ON && runs_on(&words[last], &words[last + 1]) {
        last += 1;
    }
    let text: Vec<&str> = words[first..=last]
        .iter()
        .map(|w| w.text.as_str())
        .collect();
    Some(text.join(" "))
}

/// For each "this" said, where the mouse was then, in percent of the window
/// at `rect` (left, top, right, bottom); none when it was off the window or
/// not seen near then.
fn targets(said: &[Spoken], pointer: &[(u64, i32, i32)], rect: [i32; 4]) -> Vec<(u64, f32, f32)> {
    let [left, top, right, bottom] = rect;
    let (width, height) = ((right - left) as f32, (bottom - top) as f32);
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    said.iter()
        .filter(|s| is_deictic(&s.text))
        .filter_map(|s| {
            let &(_, x, y) = pointer
                .iter()
                .filter(|(at, ..)| at.abs_diff(s.at) <= POINTER_GAP_MS)
                .min_by_key(|(at, ..)| at.abs_diff(s.at))?;
            let x = (x - left) as f32 / width * 100.0;
            let y = (y - top) as f32 / height * 100.0;
            ((0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y)).then_some((s.at, x, y))
        })
        .collect()
}

/// What the mouse was on at each "this" said, from `page`, the words on
/// the screenshot of the window at `rect` taken as the key went down.
pub fn pointed(
    said: &[Spoken],
    pointer: &[(u64, i32, i32)],
    rect: [i32; 4],
    page: &Page,
) -> Vec<Event> {
    targets(said, pointer, rect)
        .into_iter()
        .filter_map(|(at, x, y)| {
            Some(Event {
                at,
                what: What::Pointed {
                    label: under(page, x, y)?,
                },
            })
        })
        .collect()
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
fn recognize(bgra: &[u8], width: u32, height: u32) -> windows::core::Result<Page> {
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
    let mut words = Vec::new();
    for (n, line) in result.Lines()?.into_iter().enumerate() {
        for word in line.Words()? {
            let r = word.BoundingRect()?;
            words.push(Word {
                text: word.Text()?.to_string_lossy(),
                x0: r.X,
                x1: r.X + r.Width,
                y0: r.Y,
                y1: r.Y + r.Height,
                line: n as u32,
            });
        }
    }
    Ok(Page {
        width: width as f32,
        height: height as f32,
        words,
    })
}

#[cfg(not(windows))]
fn recognize(_bgra: &[u8], _width: u32, _height: u32) -> Result<Page, String> {
    Err("OCR is only on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::timeline::Region;

    fn word(text: &str, line: u32, x0: f32, x1: f32, y0: f32, y1: f32) -> Word {
        Word {
            text: text.into(),
            x0,
            x1,
            y0,
            y1,
            line,
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

    /// 100 pixels square, so percent and pixels agree.
    fn page() -> Page {
        Page {
            width: 100.0,
            height: 100.0,
            words: vec![
                word("queue", 0, 10.0, 18.0, 10.0, 14.0),
                word("full,", 0, 19.0, 25.0, 10.0, 14.0),
                word("dropping", 0, 26.0, 38.0, 10.0, 14.0),
                word("2", 0, 39.0, 41.0, 10.0, 14.0),
                word("stream", 1, 10.0, 19.0, 40.0, 44.0),
                word("done", 1, 20.0, 27.0, 40.0, 44.0),
            ],
        }
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

    /// A toolbar, then a row of buttons, on a 1000 x 500 window.
    fn toolbar() -> Page {
        Page {
            width: 1000.0,
            height: 500.0,
            words: vec![
                word("File", 0, 10.0, 40.0, 10.0, 30.0),
                word("Edit", 0, 80.0, 110.0, 10.0, 30.0),
                word("Run", 1, 10.0, 40.0, 100.0, 120.0),
                word("tests", 1, 46.0, 90.0, 100.0, 120.0),
                word("Cancel", 1, 140.0, 200.0, 100.0, 120.0),
            ],
        }
    }

    #[test]
    fn the_mouse_is_on_a_word_and_the_words_running_on_from_it() {
        // On "tests": "Run" is a space away, "Cancel" well apart.
        assert_eq!(under(&toolbar(), 6.8, 22.0).as_deref(), Some("Run tests"));
        assert_eq!(under(&toolbar(), 2.5, 4.0).as_deref(), Some("File"));
        assert_eq!(under(&toolbar(), 50.0, 80.0), None);
    }

    #[test]
    fn each_this_finds_where_the_mouse_was_then() {
        let said = |at: u64, text: &str| Spoken {
            at,
            text: text.into(),
        };
        let words = [
            said(1_000, "what's"),
            said(1_300, "this?"),
            said(2_000, "that"),
            said(3_000, "here"),
        ];
        let pointer = [(900, 150, 250), (1_350, 300, 400), (2_000, 5_000, 5_000)];
        // The window is 1000 x 500 at (100, 200). "that" was off it, and
        // nobody saw the mouse near "here".
        assert_eq!(
            targets(&words, &pointer, [100, 200, 1_100, 700]),
            [(1_300, 20.0, 40.0)]
        );
        assert!(targets(&words, &pointer, [0, 0, 0, 0]).is_empty());
    }

    #[test]
    fn a_page_reads_line_by_line() {
        assert_eq!(toolbar().text(), "File Edit\nRun tests Cancel");
        assert_eq!(Page::default().text(), "");
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
            Ok(page) => page.words,
            // No OCR language installed here: nothing to check.
            Err(_) => return,
        };
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(text, ["queue", "full", "dropping"]);
        assert!(words[0].x0 < words[1].x0 && words[1].x0 < words[2].x0);
        assert!(words
            .iter()
            .all(|w| w.line == 0 && w.y0 > 10.0 && w.y1 < 100.0));
    }
}
