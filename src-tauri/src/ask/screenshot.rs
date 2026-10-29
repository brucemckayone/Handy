//! A screenshot of the window a question is asked from, taken as the Ask
//! shortcut goes down (that window is in front then). Saved as a PNG for
//! Claude Code to read, and deleted once the answer is done.

use std::path::{Path, PathBuf};

/// Wider shots are scaled down: plenty to read, and fewer tokens.
const MAX_WIDTH: u32 = 1600;

/// Where screenshots wait for their question.
pub fn dir() -> PathBuf {
    std::env::temp_dir().join("handy-ask")
}

/// RGB pixels, `width * height * 3` bytes, rows top first.
struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// Shrinks by a whole factor so the width is at most `max_width`, averaging
/// each block of pixels.
fn shrink(image: Image, max_width: u32) -> Image {
    let factor = image.width.div_ceil(max_width).max(1);
    if factor == 1 {
        return image;
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

fn save_png(image: &Image, path: &Path) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgb)?;
    Ok(())
}

/// Captures window `handle` into a new PNG in [`dir`].
pub fn capture(handle: isize) -> Option<PathBuf> {
    let image = grab(handle)?;
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
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = HWND(handle as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    // SAFETY: every GDI object made here is released before returning; the
    // pixel buffer is sized for the bitmap GetDIBits fills.
    unsafe {
        GetWindowRect(hwnd, &mut rect).ok()?;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if width <= 0 || height <= 0 {
            return None;
        }
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
            rect.left,
            rect.top,
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
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let rows = if copied.is_ok() {
            GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
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
        if rows != height {
            return None;
        }
        let rgb = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, _]| [r, g, b])
            .collect();
        Some(Image {
            width: width as u32,
            height: height as u32,
            rgb,
        })
    }
}

/// Copies window `handle` (its window number) with `screencapture`.
#[cfg(target_os = "macos")]
fn grab(handle: isize) -> Option<Image> {
    if !screen_capture_allowed() {
        return None;
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}-window.png", super::new_uuid()));
    // -x: no camera sound; -o: no window shadow.
    let captured = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", "-t", "png", &format!("-l{handle}")])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let image = captured.then(|| read_png(&path)).flatten();
    let _ = std::fs::remove_file(&path);
    image
}

/// Whether Handy may record the screen. Without it other apps' windows come
/// out blank, so the first refusal asks (macOS shows its prompt once) and
/// questions go without a screenshot until it is granted.
#[cfg(target_os = "macos")]
fn screen_capture_allowed() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    // SAFETY: permission queries that take no arguments.
    let allowed = unsafe { CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() };
    if !allowed {
        log::info!("No screenshot: Screen Recording isn't allowed for Handy");
    }
    allowed
}

/// A PNG's pixels as [`Image`], dropping any alpha.
#[cfg(any(target_os = "macos", test))]
fn read_png(path: &Path) -> Option<Image> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgb = match info.color_type {
        png::ColorType::Rgb => pixels.to_vec(),
        png::ColorType::Rgba => pixels
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0]])
            .collect(),
        // normalize_to_color8 expands palettes.
        png::ColorType::Indexed => return None,
    };
    Some(Image {
        width: info.width,
        height: info.height,
        rgb,
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn grab(_handle: isize) -> Option<Image> {
    None
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
            Image {
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
            Image {
                width: 3,
                height: 1,
                rgb: vec![1; 9],
            },
            1600,
        );
        assert_eq!(image.width, 3);
    }

    #[test]
    fn reads_a_png_dropping_alpha() {
        let path = std::env::temp_dir().join(format!("handy-read-png-{}.png", std::process::id()));
        {
            let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
            let mut encoder = png::Encoder::new(file, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255, 0, 0, 255, 0])
                .unwrap();
        }
        let image = read_png(&path);
        let _ = std::fs::remove_file(&path);
        let image = image.expect("decoded");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgb, [255, 0, 0, 0, 0, 255]);
    }

    /// Live: in a desktop session with Screen Recording allowed for the
    /// terminal (`cargo test -- --ignored live`), captures the window in front.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_captures_the_window_in_front() {
        let window = super::super::window::foreground().expect("a window in front");
        let path = capture(window.handle).expect("a screenshot");
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        println!("{} bytes from {}", bytes.len(), window.label());
        assert_eq!(&bytes[1..4], b"PNG");
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
