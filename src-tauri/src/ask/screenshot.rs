//! A screenshot of the window a question is asked from, taken as the Ask
//! shortcut goes down (that window is in front then). Saved as a PNG for
//! Claude Code to read, and deleted once the answer is done.

use std::path::{Path, PathBuf};

/// Wider shots are scaled down: plenty to read, and fewer tokens.
const MAX_WIDTH: u32 = 1600;
/// The pill's thumbnails, at twice their size for high-DPI screens.
const THUMBNAIL_WIDTH: u32 = 96;

/// Where screenshots wait for their question.
pub fn dir() -> PathBuf {
    std::env::temp_dir().join("handy-ask")
}

/// RGB pixels, `width * height * 3` bytes, rows top first.
#[derive(Clone)]
struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// Shrinks by a whole factor so the width is at most `max_width`, averaging
/// each block of pixels.
fn shrink(image: &Image, max_width: u32) -> Image {
    let factor = image.width.div_ceil(max_width).max(1);
    if factor == 1 {
        return image.clone();
    }
    let (w, h) = (image.width / factor, image.height / factor);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let count = factor * factor;
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            for dy in 0..factor {
                let row = ((y * factor + dy) * image.width + x * factor) as usize * 3;
                for dx in 0..factor as usize {
                    for (c, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.rgb[row + dx * 3 + c]);
                    }
                }
            }
            rgb.extend(sum.iter().map(|total| (total / count) as u8));
        }
    }
    Image {
        width: w,
        height: h,
        rgb,
    }
}

fn write_png(image: &Image, out: impl std::io::Write) -> anyhow::Result<()> {
    let mut encoder = png::Encoder::new(out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgb)?;
    Ok(())
}

fn save_png(image: &Image, path: &Path) -> anyhow::Result<()> {
    write_png(image, std::io::BufWriter::new(std::fs::File::create(path)?))
}

/// A small copy as a PNG data URL, for the pill.
fn thumbnail(image: &Image) -> Option<String> {
    use base64::Engine as _;
    let mut png = Vec::new();
    write_png(&shrink(image, THUMBNAIL_WIDTH), &mut png).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    Some(format!("data:image/png;base64,{data}"))
}

/// Captures window `handle` into a new PNG in [`dir`].
pub fn capture(handle: isize) -> Option<PathBuf> {
    save(&grab(handle)?)
}

/// Captures window `handle` into a new PNG, with a thumbnail of it.
pub fn capture_with_thumbnail(handle: isize) -> Option<(PathBuf, String)> {
    let image = grab(handle)?;
    let thumbnail = thumbnail(&image)?;
    Some((save(&image)?, thumbnail))
}

fn save(image: &Image) -> Option<PathBuf> {
    let image = shrink(image, MAX_WIDTH);
    let dir = dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.png", super::new_uuid()));
    match save_png(&image, &path) {
        Ok(()) => Some(path),
        Err(e) => {
            log::warn!("Couldn't save the screenshot: {e}");
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Copies the window's area of the screen.
#[cfg(windows)]
fn grab(handle: isize) -> Option<Image> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = HWND(handle as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    // SAFETY: a plain query; the rect outlives the call that fills it.
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    let bgra = copy_screen(
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    )?;
    let rgb = bgra
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, _]| [r, g, b])
        .collect();
    Some(Image {
        width: bgra.width,
        height: bgra.height,
        rgb,
    })
}

#[cfg(not(windows))]
fn grab(_handle: isize) -> Option<Image> {
    None
}

/// Screen pixels as GDI gives them: blue, green, red, unused; rows top first.
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Copies part of the screen, in physical pixels of the virtual desktop.
#[cfg(windows)]
pub fn copy_screen(left: i32, top: i32, width: i32, height: i32) -> Option<Bgra> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };

    if width <= 0 || height <= 0 {
        return None;
    }
    // SAFETY: every GDI object made here is released before returning; the
    // pixel buffer is sized for the bitmap GetDIBits fills.
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            left,
            top,
            SRCCOPY | CAPTUREBLT,
        );
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top first.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let rows = if copied.is_ok() {
            GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        (rows == height).then_some(Bgra {
            width: width as u32,
            height: height as u32,
            pixels,
        })
    }
}

#[cfg(not(windows))]
pub fn copy_screen(_left: i32, _top: i32, _width: i32, _height: i32) -> Option<Bgra> {
    None
}

/// An uncompressed 32-bit BMP, rows top first: written in no time even for
/// a 4K screen, and any webview shows it. Alpha is set opaque, as GDI
/// leaves it zero.
fn bmp(image: &Bgra) -> Vec<u8> {
    let data = image.pixels.len() as u32;
    let mut out = Vec::with_capacity(54 + data as usize);
    out.extend(b"BM");
    out.extend((54 + data).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend((image.width as i32).to_le_bytes());
    out.extend((-(image.height as i32)).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(data.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    for pixel in image.pixels.as_chunks::<4>().0 {
        out.extend([pixel[0], pixel[1], pixel[2], 255]);
    }
    out
}

/// Saves `image` as a BMP in [`dir`], for the grab overlay to show.
pub fn save_frame(image: &Bgra) -> Option<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.bmp", super::new_uuid()));
    match std::fs::write(&path, bmp(image)) {
        Ok(()) => Some(path),
        Err(e) => {
            log::warn!("Couldn't save the frozen screen: {e}");
            None
        }
    }
}

/// Saves a PNG made elsewhere (the grab overlay's marked-up picture) in [`dir`].
pub fn save_png_bytes(png: &[u8]) -> Option<PathBuf> {
    if png.get(1..4) != Some(b"PNG") {
        return None;
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.png", super::new_uuid()));
    std::fs::write(&path, png).ok()?;
    Some(path)
}

/// A PNG's size, from its header: a capture marked up again is shown at it.
pub fn png_size(path: &Path) -> Option<(u32, u32)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let reader = png::Decoder::new(file).read_info().ok()?;
    let info = reader.info();
    Some((info.width, info.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_by_a_whole_factor_averaging_blocks() {
        // 4x2: left half black, right half white.
        let mut rgb = Vec::new();
        for _ in 0..2 {
            rgb.extend([0u8; 6]);
            rgb.extend([255u8; 6]);
        }
        let small = shrink(
            &Image {
                width: 4,
                height: 2,
                rgb,
            },
            2,
        );
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.rgb, [0, 0, 0, 255, 255, 255]);
    }

    #[test]
    fn small_images_are_kept() {
        let image = shrink(
            &Image {
                width: 3,
                height: 1,
                rgb: vec![1; 9],
            },
            1600,
        );
        assert_eq!(image.width, 3);
    }

    #[test]
    fn frames_are_opaque_top_down_bmps() {
        let frame = bmp(&Bgra {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 0, 4, 5, 6, 0],
        });
        assert_eq!(&frame[..2], b"BM");
        assert_eq!(frame.len(), 54 + 8);
        assert_eq!(u32::from_le_bytes(frame[2..6].try_into().unwrap()), 62);
        assert_eq!(i32::from_le_bytes(frame[22..26].try_into().unwrap()), -1);
        assert_eq!(&frame[54..], [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn only_pngs_are_saved_as_pngs() {
        assert!(save_png_bytes(b"not a picture").is_none());
    }

    #[test]
    fn png_sizes_come_from_the_header() {
        let mut png = Vec::new();
        let image = Image {
            width: 5,
            height: 3,
            rgb: vec![9; 45],
        };
        write_png(&image, &mut png).unwrap();
        let path = save_png_bytes(&png).expect("saved");
        assert_eq!(png_size(&path), Some((5, 3)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn thumbnails_are_small_pngs_in_a_data_url() {
        use base64::Engine as _;
        let image = Image {
            width: 1000,
            height: 600,
            rgb: vec![200; 1000 * 600 * 3],
        };
        let url = thumbnail(&image).expect("a thumbnail");
        let data = url
            .strip_prefix("data:image/png;base64,")
            .expect("a PNG data URL");
        let png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let info = reader.info();
        assert!(info.width <= THUMBNAIL_WIDTH, "{} wide", info.width);
        assert_eq!(info.width * 6, info.height * 10, "the shape is kept");
    }

    #[cfg(windows)]
    #[test]
    fn captures_the_desktop_window() {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: a plain query.
        let desktop = unsafe { GetDesktopWindow() };
        let path = capture(desktop.0 as isize).expect("a screenshot");
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[1..4], b"PNG");
    }
}
