//! Application icons, found on disk and decoded off the drawing thread.
//!
//! A desktop entry names its icon, and the icon is a file somewhere in the
//! icon themes the desktop installs: `firefox` is
//! `/usr/share/icons/hicolor/128x128/apps/firefox.png` or
//! `/usr/share/icons/Adwaita/scalable/apps/firefox.svg`, depending on what the
//! distribution shipped. The themes are searched rather than a path being
//! guessed, since a guess is wrong on somebody's system.
//!
//! Decoding an SVG takes milliseconds, so it happens on a thread of its own:
//! the table asks for the icon of every row it draws and gets it a frame or
//! two later, which keeps a scroll through a hundred applications smooth.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use eframe::egui;

/// The side an icon is decoded at, in pixels.
///
/// Twice the size a row draws it at, so it stays sharp on a display whose
/// scale factor is 2.
const SIZE: u32 = 32;

/// How many icons are kept, after which the least recently asked for goes.
///
/// One icon is four kilobytes decoded, so this is a couple of megabytes at
/// worst, and a table of a hundred rows never reaches it.
const KEPT: usize = 512;

/// A decoded icon: its pixels, in the form a texture takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    /// The side of the square, in pixels.
    pub size: [usize; 2],
    /// The pixels, four bytes each, unmultiplied.
    pub rgba: Vec<u8>,
}

/// The icons the table draws, and the thread that decodes the rest.
#[derive(Default)]
pub struct Icons {
    /// The textures already made, by icon name.
    textures: HashMap<String, Option<egui::TextureHandle>>,
    /// The order they were last asked for, so one can make room.
    order: Vec<String>,
    /// Names handed to the thread, so each is asked for once.
    asked: Vec<String>,
    /// The thread, started when the first icon is asked for.
    worker: Option<Worker>,
    /// An icon name that no theme has, remembered so the themes are not
    /// searched again for it.
    missing: Vec<String>,
}

/// The queue to the thread that finds and decodes icons.
struct Worker {
    /// The names to look for.
    wanted: Sender<String>,
    /// The pictures that came back.
    results: Receiver<(String, Option<Image>)>,
}

impl Icons {
    /// The texture for `name`, asking for it when it is not made yet.
    ///
    /// The answer is `None` both for an icon not decoded yet and for one that
    /// does not exist, which the caller draws the same way: a placeholder.
    pub fn texture(&mut self, ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
        if let Some(found) = self.textures.get(name).cloned() {
            self.touch(name);
            return found;
        }
        if self.missing.iter().any(|missing| missing == name) {
            return None;
        }
        if !self.asked.iter().any(|asked| asked == name) {
            // The search for the file runs on the thread too: it walks the
            // icon themes, which is milliseconds that the drawing thread
            // should not spend, once per icon name either way.
            self.asked.push(name.to_string());
            self.worker(ctx).wanted.send(name.to_string()).ok();
        }
        self.take(ctx);
        self.textures.get(name).and_then(Clone::clone)
    }

    /// Takes the icons that came back and turns them into textures.
    ///
    /// Returns whether any arrived, which the caller turns into a repaint.
    pub fn take(&mut self, ctx: &egui::Context) -> bool {
        let Some(worker) = self.worker.take() else {
            return false;
        };
        let mut arrived = false;
        let mut reinsert = true;
        loop {
            match worker.results.try_recv() {
                Ok((name, image)) => {
                    match &image {
                        Some(image) => {
                            let texture = ctx.load_texture(
                                format!("icon:{name}"),
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            );
                            self.textures.insert(name.clone(), Some(texture));
                        }
                        None => {
                            self.missing.push(name.clone());
                            self.textures.insert(name.clone(), None);
                        }
                    }
                    self.touch(&name);
                    arrived = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The thread is gone, which it only does when the window
                    // is closing; there is nothing left to wait for.
                    reinsert = false;
                    break;
                }
            }
        }
        if reinsert {
            self.worker = Some(worker);
        }
        arrived
    }

    /// Remembers that `name` was asked for, dropping the oldest past [`KEPT`].
    fn touch(&mut self, name: &str) {
        self.order.retain(|known| known != name);
        self.order.push(name.to_string());
        while self.order.len() > KEPT {
            let oldest = self.order.remove(0);
            self.textures.remove(&oldest);
            self.asked.retain(|asked| *asked != oldest);
        }
    }

    /// The decoding thread, started when it is first needed.
    fn worker(&mut self, ctx: &egui::Context) -> &mut Worker {
        self.worker.get_or_insert_with(|| {
            let (wanted, wanted_receiver) = mpsc::channel::<String>();
            let (results, result_receiver) = mpsc::channel();
            let ctx = ctx.clone();
            thread::Builder::new()
                .name("icons".to_string())
                .spawn(move || {
                    while let Ok(name) = wanted_receiver.recv() {
                        let image = find(&name).and_then(|path| decode(&path));
                        if results.send((name, image)).is_err() {
                            return;
                        }
                        // The window may be idle between samples, so the
                        // decoded icon has to ask for the frame that shows it.
                        ctx.request_repaint();
                    }
                })
                .expect("the icon thread starts");
            Worker {
                wanted,
                results: result_receiver,
            }
        })
    }
}

/// The file an icon name refers to, searching the themes the desktop has.
///
/// The search is the one the icon theme specification describes, cut down to
/// what a table of rows needs: the theme the session runs, then the themes it
/// inherits, then `hicolor`, which every icon set falls back to.
pub fn find(name: &str) -> Option<PathBuf> {
    // An entry may name a path rather than a name, which is what a program
    // that ships its icon itself does.
    let direct = Path::new(name);
    if direct.is_absolute() && direct.is_file() {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    for directory in roots() {
        if let Some(path) = search(&directory, name) {
            return Some(path);
        }
    }
    None
}

/// The icon under one theme directory.
///
/// The sizes are tried from the one the row draws at outwards, since a theme
/// usually ships several and the closest is the sharpest.
fn search(theme: &Path, name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for size in [
        "32x32", "48x48", "64x64", "24x24", "128x128", "scalable", "256x256", "512x512", "16x16",
    ] {
        for context in [
            "apps",
            "applications",
            "devices",
            "places",
            "categories",
            "mimetypes",
        ] {
            // XPM is not decoded, so offering it here would hide a readable
            // PNG or SVG at the next size behind a file that can never draw.
            for extension in ["png", "svg"] {
                candidates.push(
                    theme
                        .join(size)
                        .join(context)
                        .join(format!("{name}.{extension}")),
                );
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// The icon theme directories, best first.
///
/// The user's own themes come before the system's, and the theme the session
/// names comes before the ones it inherits: a user who installed an icon set
/// gets the icons they see in their menus.
fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut bases = Vec::new();
    if let Some(data) = dirs::data_dir() {
        bases.push(data.join("icons"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            ["/usr/local/share", "/usr/share"]
                .iter()
                .map(PathBuf::from)
                .collect()
        });
    for directory in data_dirs {
        bases.push(directory.join("icons"));
    }
    let theme = std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .and_then(|desktop| theme_for(&desktop));
    for base in bases {
        if let Some(theme) = &theme {
            roots.push(base.join(theme));
        }
        for inherited in ["hicolor", "Adwaita", "breeze", "Papirus"] {
            roots.push(base.join(inherited));
        }
        // Whatever is installed, in a stable order, so a theme the list does
        // not name still answers for an icon the others lack.
        if let Ok(entries) = std::fs::read_dir(&base) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            names.sort();
            for name in names {
                roots.push(base.join(name));
            }
        }
    }
    roots
}

/// The icon theme a desktop session uses.
fn theme_for(desktop: &str) -> Option<String> {
    let desktop = desktop.to_ascii_lowercase();
    if desktop.contains("kde") {
        Some("breeze".to_string())
    } else if desktop.contains("gnome") || desktop.contains("unity") || desktop.contains("pantheon")
    {
        Some("Adwaita".to_string())
    } else {
        None
    }
}

/// Decodes an icon file, scaled to [`SIZE`].
///
/// Returns `None` for a format this does not read and for anything malformed,
/// which the caller draws as a placeholder rather than reporting.
pub fn decode(path: &Path) -> Option<Image> {
    let bytes = std::fs::read(path).ok()?;
    if looks_like_svg(&bytes) {
        return svg(&bytes);
    }
    png(&bytes)
}

/// Whether the bytes open like an SVG document.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    head.windows(4).any(|window| window == b"<svg")
}

/// Decodes a PNG and scales it down to a square of [`SIZE`].
fn png(bytes: &[u8]) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 0xff])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Grayscale => pixels
            .iter()
            .flat_map(|gray| [*gray, *gray, *gray, 0xff])
            .collect(),
        _ => return None,
    };
    Some(scale(
        Image {
            size: [frame.width as usize, frame.height as usize],
            rgba,
        },
        SIZE,
    ))
}

/// Renders an SVG at [`SIZE`].
fn svg(bytes: &[u8]) -> Option<Image> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let extent = tree.size();
    let scale = (SIZE as f32 / extent.width().max(extent.height())).min(1.0);
    let width = (extent.width() * scale).round().max(1.0) as u32;
    let height = (extent.height() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    Some(Image {
        size: [width as usize, height as usize],
        rgba,
    })
}

/// Scales an image down so neither side is more than `size`, averaging the
/// pixels that land on one another.
fn scale(image: Image, size: u32) -> Image {
    let size = size as usize;
    let [width, height] = image.size;
    if width <= size && height <= size {
        return image;
    }
    let factor_x = width as f32 / size as f32;
    let factor_y = height as f32 / size as f32;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let x0 = (x as f32 * factor_x) as usize;
            let y0 = (y as f32 * factor_y) as usize;
            let x1 = (((x + 1) as f32 * factor_x) as usize)
                .min(width)
                .max(x0 + 1);
            let y1 = (((y + 1) as f32 * factor_y) as usize)
                .min(height)
                .max(y0 + 1);
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for source_y in y0..y1 {
                for source_x in x0..x1 {
                    let offset = (source_y * width + source_x) * 4;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.rgba[offset + channel]);
                    }
                    count += 1;
                }
            }
            let count = count.max(1);
            for channel in sum {
                rgba.push((channel / count) as u8);
            }
        }
    }
    Image {
        size: [size, size],
        rgba,
    }
}

/// The icon drawn when an application has none of its own: a square with the
/// first letter of its name.
pub struct Placeholder;

impl Placeholder {
    /// Paints a placeholder icon into `rect`: a faint rounded square with the
    /// first letter of the name, which reads as an icon without pretending to
    /// be one.
    pub fn paint(
        painter: &egui::Painter,
        rect: egui::Rect,
        letter: char,
        color: egui::Color32,
        on: egui::Color32,
    ) {
        painter.rect_filled(rect, 4.0, color);
        let font = egui::FontId::proportional(rect.height() * 0.72);
        let text = painter.layout_no_wrap(letter.to_uppercase().to_string(), font, on);
        let position = rect.center() - text.size() / 2.0;
        painter.galley(position, text, on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_is_rendered_at_the_size_a_row_needs() {
        let svg: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
            <rect width="64" height="64" fill="#4cd8a4"/>
        </svg>"##;
        let image = svg_decode(svg).expect("the svg renders");
        assert_eq!(image.size, [SIZE as usize, SIZE as usize]);
        assert_eq!(image.rgba.len(), (SIZE * SIZE * 4) as usize);
        // The middle of the square is the colour it was filled with.
        let middle = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        assert_eq!(&image.rgba[middle..middle + 3], [0x4c, 0xd8, 0xa4]);
    }

    /// Renders an SVG, which is what [`decode`] does for one.
    fn svg_decode(bytes: &[u8]) -> Option<Image> {
        svg(bytes)
    }

    #[test]
    fn a_svg_is_recognised_by_its_opening() {
        assert!(looks_like_svg(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"
        ));
        assert!(!looks_like_svg(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn a_large_image_is_scaled_down_and_a_small_one_is_left_alone() {
        let big = Image {
            size: [64, 64],
            rgba: vec![200; 64 * 64 * 4],
        };
        let scaled = scale(big, 16);
        assert_eq!(scaled.size, [16, 16]);
        assert_eq!(scaled.rgba.len(), 16 * 16 * 4);
        // Averaging a field of one colour gives that colour back.
        assert!(scaled.rgba.iter().take(4).all(|byte| *byte == 200));

        let small = Image {
            size: [8, 8],
            rgba: vec![7; 8 * 8 * 4],
        };
        assert_eq!(scale(small.clone(), 16), small);
    }

    #[test]
    fn a_png_is_decoded() {
        // A 2x2 PNG, written out by the encoder rather than kept as bytes.
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("the header is written");
            writer
                .write_image_data(&[
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
                ])
                .expect("the pixels are written");
        }
        let image = png(&bytes).expect("the png decodes");
        assert_eq!(image.size, [2, 2]);
        assert_eq!(&image.rgba[..4], [255, 0, 0, 255]);
    }

    #[test]
    fn a_file_that_is_not_an_image_decodes_to_nothing() {
        let plain = png(b"not a png at all");
        assert!(plain.is_none());
        let not_svg = svg(b"<html></html>");
        assert!(not_svg.is_none());
    }

    #[test]
    fn searching_for_an_icon_is_quick() {
        let started = std::time::Instant::now();
        let _ = find("utilities-terminal");
        println!("an icon search takes {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let _ = find("no-such-icon-anywhere");
        println!("a missing icon search takes {:?}", started.elapsed());
    }

    #[test]
    fn a_real_desktop_icon_is_found_and_decoded() {
        // Whatever the machine has, take the first desktop entry that names an
        // icon and follow it all the way to pixels. This is the path a row
        // takes, and it must end in an image rather than a placeholder.
        let index = crate::desktop::Index::load();
        let mut tried = 0;
        for entry in index.entries() {
            if entry.icon.is_empty() || entry.icon.starts_with('/') {
                continue;
            }
            tried += 1;
            if tried > 8 {
                break;
            }
            let path = find(&entry.icon);
            println!(
                "{}: icon {:?} -> {:?}",
                entry.path.display(),
                entry.icon,
                path
            );
            if let Some(path) = path {
                let image = decode(&path);
                println!(
                    "   decoded: {:?}",
                    image.as_ref().map(|i| (i.size, i.rgba.len()))
                );
                assert!(image.is_some(), "{} did not decode", path.display());
                return;
            }
        }
        if tried == 0 {
            // A machine with no desktop entries at all has nothing to test.
            return;
        }
        panic!("none of {tried} desktop entries' icons could be found");
    }

    #[test]
    fn the_system_has_an_icon_theme_to_search() {
        // Whatever the test machine has, an icon name a desktop entry is
        // likely to use is either found or reported missing, never a panic.
        let _ = find("utilities-terminal");
        let _ = find("no-such-icon-anywhere");
        let _ = find("/no/such/path.png");
    }

    #[test]
    fn a_decoded_icon_asks_for_the_frame_that_shows_it() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let directory = std::env::temp_dir().join(format!(
            "process-control-icon-repaint-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("the directory is made");
        let path = directory.join("thing.png");
        {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("the header is written");
                writer
                    .write_image_data(&[255, 255, 255, 255])
                    .expect("the pixel is written");
            }
            std::fs::write(&path, bytes).expect("the png is written");
        }
        let ctx = egui::Context::default();
        let arrived = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&arrived);
        ctx.set_request_repaint_callback(move |_| flag.store(true, Ordering::SeqCst));
        let mut icons = Icons::default();
        let _ = icons.texture(&ctx, &path.to_string_lossy());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !arrived.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert!(
            arrived.load(Ordering::SeqCst),
            "the worker asks the window to redraw when the icon arrives"
        );
    }

    #[test]
    fn an_unreadable_xpm_does_not_hide_a_png_at_another_size() {
        let directory =
            std::env::temp_dir().join(format!("process-control-icon-theme-{}", std::process::id()));
        let small = directory.join("32x32/apps");
        let larger = directory.join("48x48/apps");
        std::fs::create_dir_all(&small).expect("the small directory is made");
        std::fs::create_dir_all(&larger).expect("the larger directory is made");
        std::fs::write(small.join("thing.xpm"), "not decoded").expect("the xpm is written");
        std::fs::write(larger.join("thing.png"), "not decoded either").expect("the png is written");
        let found = search(&directory, "thing").expect("the png is found");
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert_eq!(found, larger.join("thing.png"));
    }

    #[test]
    fn a_file_path_is_its_own_answer() {
        let directory =
            std::env::temp_dir().join(format!("process-control-icon-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the directory is made");
        let path = directory.join("mine.svg");
        std::fs::write(
            &path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"></svg>",
        )
        .expect("the file is written");
        let found = find(&path.to_string_lossy());
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert_eq!(found, Some(path));
    }
}
