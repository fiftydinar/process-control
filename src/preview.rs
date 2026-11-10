//! Drawing one frame of the window into a picture, for looking at it.
//!
//! egui hands the renderer a list of triangles with a colour at each corner,
//! and a GPU blends them. A machine with no display server and no working GPU
//! cannot run the window at all, so this fills the triangles itself with
//! tiny-skia, which resvg already brings in for SVG icons. Each triangle is
//! filled with the average of its three corners, which is exact for the solid
//! shapes egui draws and close enough for a glyph to be read.
//!
//! This exists so a change can be looked at on a headless machine. It is not
//! part of the app, and only the tests build it.

use eframe::egui;
use resvg::tiny_skia;

/// What the picture is drawn in, and what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// Whether the picture is drawn in the app's own dark palette.
    pub dark: bool,
    /// Whether the picture shows the process tree rather than the
    /// applications.
    pub tree: bool,
    /// Whether the picture has the details panel open.
    pub details: bool,
    /// Whether the applications section is folded away.
    pub folded: bool,
}

impl Look {
    /// The window as it opens, in the app's own palette.
    pub const DARK: Self = Self {
        dark: true,
        tree: false,
        details: false,
        folded: false,
    };

    /// The same in the light palette.
    pub const LIGHT: Self = Self {
        dark: false,
        ..Self::DARK
    };

    /// The process tree, with an application's helpers folded away.
    pub const TREE: Self = Self {
        tree: true,
        ..Self::DARK
    };

    /// The details panel, open on a process.
    pub const DETAILS: Self = Self {
        details: true,
        ..Self::DARK
    };

    /// The applications section folded away, leaving its heading.
    pub const FOLDED: Self = Self {
        folded: true,
        ..Self::DARK
    };
}

/// Draws one frame of the window into `pixmap`.
pub fn render(pixmap: &mut tiny_skia::Pixmap, look: Look) {
    let width = pixmap.width() as f32;
    let height = pixmap.height() as f32;
    let ctx = egui::Context::default();
    let theme = if look.dark {
        crate::theme::Theme::Default
    } else {
        crate::theme::Theme::AdwaitaLight
    };
    crate::fonts::install(&ctx);

    // The app applies the theme it read from the settings, so the picture's
    // theme is applied after it rather than before.
    let mut app = crate::app::App::new(&ctx, true);
    crate::theme::apply(&ctx, theme);
    app.sample = sample();
    app.applications =
        crate::apps::Applications::new(&app.sample, &app.sample.windows, &app.entries);
    app.applications.resolve(&app.sample);
    if look.tree {
        app.view = crate::apps::ViewMode::Tree;
    }
    if look.folded {
        app.fold_section_for_preview(crate::apps::Section::Applications);
    }
    app.rebuild();
    if look.details {
        let position = app
            .rows
            .iter()
            .position(|row| row.name.starts_with("firefox"))
            .or_else(|| (!app.rows.is_empty()).then_some(0));
        if let Some(position) = position {
            app.open_details(position);
        }
    }
    app.push_histories_for_preview();
    app.rebuild();

    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..Default::default()
    };
    let _ = eframe::Frame::_new_kittest();
    let output = ctx.run_ui(input, |ui| crate::ui::draw(&mut app, ui));
    let atlases = atlases(&output.textures_delta);
    let primitives = ctx.tessellate(output.shapes, 1.0);
    fill(pixmap, &primitives, &atlases);
    // The textures were sampled above; egui insists the delta is not dropped
    // silently, and this is the acknowledgement.
    let mut delta = output.textures_delta;
    delta.clear();
}

/// A texture egui handed over this frame: its pixels, ready to sample.
struct Atlas {
    /// The texture's id, which a mesh names.
    id: egui::TextureId,
    /// The side of the texture, in pixels.
    size: [usize; 2],
    /// The pixels, four bytes each.
    rgba: Vec<u8>,
}

/// Collects the textures egui made this frame: its font atlas, and any
/// application icon it uploaded.
///
/// The atlas grows as glyphs are laid out, so one frame brings a whole texture
/// and then patches that add to it; each patch is written where it says it
/// goes, which is what a GPU would do.
fn atlases(delta: &egui::TexturesDelta) -> Vec<Atlas> {
    let mut atlases: Vec<Atlas> = Vec::new();
    for (id, patches) in &delta.set {
        for patch in patches {
            let egui::ImageData::Color(image) = &patch.image;
            let pixels: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                .collect();
            match atlases.iter_mut().find(|atlas| atlas.id == *id) {
                Some(atlas) => {
                    let [at, left] = patch.pos.unwrap_or([0, 0]);
                    for row in 0..image.size[1] {
                        let destination = ((at + row) * atlas.size[0] + left) * 4;
                        let source = row * image.size[0] * 4;
                        let length = image.size[0] * 4;
                        if let Some(target) = atlas.rgba.get_mut(destination..destination + length)
                        {
                            target.copy_from_slice(&pixels[source..source + length]);
                        }
                    }
                }
                None => atlases.push(Atlas {
                    id: *id,
                    size: image.size,
                    rgba: pixels,
                }),
            }
        }
    }
    atlases
}

/// Rasterises every clipped triangle of `primitives` into `pixmap`.
///
/// Each pixel inside a triangle takes the colour and the texture coordinate
/// interpolated from the three corners, which is what a GPU does; the texture
/// coordinate then samples the atlas. That is enough to draw egui's glyphs and
/// its solid shapes, which is the whole of what this window is made of.
fn fill(pixmap: &mut tiny_skia::Pixmap, primitives: &[egui::ClippedPrimitive], atlases: &[Atlas]) {
    let width = pixmap.width() as usize;
    let height = pixmap.height() as usize;
    let data = pixmap.data_mut();
    for clipped in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let atlas = atlases.iter().find(|atlas| atlas.id == mesh.texture_id);
        let clip = clipped.clip_rect;
        let clip_min_x = clip.left().max(0.0) as usize;
        let clip_min_y = clip.top().max(0.0) as usize;
        let clip_max_x = (clip.right().ceil() as usize).min(width);
        let clip_max_y = (clip.bottom().ceil() as usize).min(height);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let corners = [
                &mesh.vertices[triangle[0] as usize],
                &mesh.vertices[triangle[1] as usize],
                &mesh.vertices[triangle[2] as usize],
            ];
            let min_x = corners
                .iter()
                .map(|corner| corner.pos.x)
                .fold(f32::MAX, f32::min)
                .max(0.0) as usize;
            let max_x = (corners
                .iter()
                .map(|corner| corner.pos.x)
                .fold(f32::MIN, f32::max)
                .ceil() as usize)
                .min(width);
            let min_y = corners
                .iter()
                .map(|corner| corner.pos.y)
                .fold(f32::MAX, f32::min)
                .max(0.0) as usize;
            let max_y = (corners
                .iter()
                .map(|corner| corner.pos.y)
                .fold(f32::MIN, f32::max)
                .ceil() as usize)
                .min(height);
            let [a, b, c] = [corners[0], corners[1], corners[2]];
            let area = (b.pos.x - a.pos.x) * (c.pos.y - a.pos.y)
                - (c.pos.x - a.pos.x) * (b.pos.y - a.pos.y);
            if area.abs() < 1.0e-6 {
                continue;
            }
            for y in min_y.max(clip_min_y)..max_y.min(clip_max_y) {
                for x in min_x.max(clip_min_x)..max_x.min(clip_max_x) {
                    // Sample at the centre of the pixel.
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let w0 =
                        ((b.pos.x - px) * (c.pos.y - py) - (c.pos.x - px) * (b.pos.y - py)) / area;
                    let w1 =
                        ((c.pos.x - px) * (a.pos.y - py) - (a.pos.x - px) * (c.pos.y - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                        continue;
                    }
                    let mut rgba = [0.0f32; 4];
                    for (weight, corner) in [(w0, a), (w1, b), (w2, c)] {
                        let [r, g, b_, alpha] = corner.color.to_srgba_unmultiplied();
                        rgba[0] += weight * f32::from(r);
                        rgba[1] += weight * f32::from(g);
                        rgba[2] += weight * f32::from(b_);
                        rgba[3] += weight * f32::from(alpha);
                    }
                    let mut coverage = 1.0f32;
                    if let Some(atlas) = atlas {
                        // The texture coordinate picks the texel; its alpha is
                        // what a glyph's shape is.
                        let u = w0 * a.uv.x + w1 * b.uv.x + w2 * c.uv.x;
                        let v = w0 * a.uv.y + w1 * b.uv.y + w2 * c.uv.y;
                        let texel_x = ((u * atlas.size[0] as f32) as isize)
                            .clamp(0, atlas.size[0] as isize - 1)
                            as usize;
                        let texel_y = ((v * atlas.size[1] as f32) as isize)
                            .clamp(0, atlas.size[1] as isize - 1)
                            as usize;
                        let offset = (texel_y * atlas.size[0] + texel_x) * 4;
                        coverage = f32::from(atlas.rgba[offset + 3]) / 255.0;
                        // A textured glyph carries a white colour and its
                        // shape in the alpha, so what is blended is the colour
                        // the mesh gave, at that coverage.
                    }
                    if coverage <= 0.004 {
                        continue;
                    }
                    let alpha = (rgba[3] / 255.0) * coverage;
                    if alpha <= 0.004 {
                        continue;
                    }
                    blend(data, width, x, y, rgba, alpha);
                }
            }
        }
    }
}

/// Blends one source colour over the pixel at `x`, `y`.
fn blend(data: &mut [u8], width: usize, x: usize, y: usize, source: [f32; 4], alpha: f32) {
    let offset = (y * width + x) * 4;
    let Some(pixel) = data.get_mut(offset..offset + 4) else {
        return;
    };
    for channel in 0..3 {
        pixel[channel] = (source[channel] * alpha + f32::from(pixel[channel]) * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    pixel[3] = 255;
}

/// A sample of a busy machine, so the picture has something to show.
fn sample() -> crate::sample::Sample {
    use crate::sample::ProcessInfo;
    let mut processes = Vec::new();
    let rows = [
        ("init", 1, 0, 'S', 0.0, 12, 1_500_000u64),
        ("systemd-journald", 412, 1, 'S', 0.6, 5, 42_000_000),
        ("pipewire", 903, 1, 'S', 1.4, 2, 18_000_000),
        ("kwin_wayland", 1180, 903, 'R', 18.7, 12, 412_000_000),
        ("firefox", 2201, 1180, 'R', 26.4, 42, 1_820_000_000),
        ("firefox", 2260, 2201, 'S', 3.1, 21, 340_000_000),
        ("firefox", 2311, 2201, 'S', 1.8, 19, 296_000_000),
        ("WebExtensions", 2388, 2201, 'S', 0.4, 8, 120_000_000),
        ("kitty", 3102, 1, 'R', 4.2, 6, 96_000_000),
        ("nvim", 3155, 3102, 'R', 2.9, 4, 78_000_000),
        ("cargo", 3401, 3155, 'R', 64.8, 22, 2_450_000_000),
        ("rustc", 3450, 3401, 'R', 41.2, 9, 1_980_000_000),
        ("process-control", 3901, 1, 'R', 1.2, 4, 34_000_000),
        ("udisksd", 780, 1, 'S', 0.0, 3, 9_000_000),
        ("kworker/0:2", 211, 2, 'I', 0.1, 1, 0),
    ];
    for (name, pid, ppid, state, cpu, threads, memory) in rows {
        processes.push(ProcessInfo {
            pid,
            ppid,
            name: name.to_string(),
            exe: if memory == 0 {
                None
            } else {
                Some(format!("/usr/bin/{name}"))
            },
            uid: 1000,
            state,
            threads,
            cpu_time: f64::from(threads) * 12.0,
            cpu,
            memory,
            virtual_memory: memory * 6,
            disk_read: if name == "cargo" { 2_600_000.0 } else { 0.0 },
            disk_write: if name == "rustc" { 940_000.0 } else { 0.0 },
            net_read: if name == "firefox" { 480_000.0 } else { 0.0 },
            net_write: if name == "firefox" { 96_000.0 } else { 0.0 },
            gpu: if name == "firefox" { 6.2 } else { 0.0 },
            gpu_memory: 0,
            started: Some(std::time::SystemTime::now()),
        });
    }
    crate::sample::Sample {
        process_count: processes.len(),
        running: processes.iter().filter(|p| p.state == 'R').count(),
        processes,
        cpu: 38.4,
        cores: vec![62.0, 41.0, 88.0, 12.0, 4.0, 30.0, 51.0, 22.0],
        memory_used: 7_400_000_000,
        memory_total: 16_000_000_000,
        swap_used: 210_000_000,
        swap_total: 8_000_000_000,
        disk_read: 3_540_000.0,
        disk_write: 1_120_000.0,
        net_read: 576_000.0,
        net_write: 128_000.0,
        gpu: Some(14.0),
        gpu_memory: 512_000_000,
        cpu_count: 8,
        uptime: 91_400,
        load: [1.42, 0.98, 0.74],
        elapsed: 1.0,
        windows: vec![
            window(2201, "Navigator", "firefox", "Mozilla Firefox"),
            window(3102, "kitty", "kitty", "nvim"),
            window(1180, "kwin_wayland", "kwin_wayland", "Plasma"),
            window(
                3901,
                "process-control",
                "process-control",
                "Process Control",
            ),
        ],
        error: None,
    }
}

/// A window a program has open.
fn window(pid: i32, class: &str, instance: &str, title: &str) -> crate::sample::Window {
    crate::sample::Window {
        id: pid as u32,
        pid,
        class: class.to_string(),
        instance: instance.to_string(),
        title: title.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_shows_every_process_of_the_sample() {
        // A sample of a busy machine has processes at every depth; the tree
        // must hold all of them, folded or not.
        let ctx = egui::Context::default();
        let mut app = crate::app::App::new(&ctx, true);
        app.sample = sample();
        app.applications =
            crate::apps::Applications::new(&app.sample, &app.sample.windows, &app.entries);
        app.applications.resolve(&app.sample);
        app.view = crate::apps::ViewMode::Tree;
        app.rebuild();
        let shown: Vec<&str> = app.rows.iter().map(|row| row.name.as_str()).collect();
        println!("{shown:?}");
        for process in &app.sample.processes {
            assert!(
                shown.iter().any(|name| *name == process.name)
                    || app.rows.iter().any(|row| row.has_children && row.folded),
                "{} is in the tree or folded away",
                process.name
            );
        }
    }

    #[test]
    #[ignore = "writes the pictures the docs use"]
    fn write_the_screenshots() {
        // The docs' screenshot is the app's own drawing, so it cannot drift
        // from what the app shows. The window is the size it opens at.
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/public");
        std::fs::create_dir_all(&docs).expect("the docs directory is made");
        for (look, name) in [
            (Look::DARK, "screenshot.png"),
            (Look::LIGHT, "screenshot-light.png"),
        ] {
            let mut pixmap = tiny_skia::Pixmap::new(1280, 800).expect("the pixmap is made");
            render(&mut pixmap, look);
            let path = docs.join(name);
            pixmap.save_png(&path).expect("the picture is written");
            println!("{}", path.display());
        }
        // Two more for a look during development.
        for (look, name) in [
            (Look::TREE, "process-control-tree.png"),
            (Look::DETAILS, "process-control-details.png"),
            (Look::FOLDED, "process-control-folded.png"),
        ] {
            let mut pixmap = tiny_skia::Pixmap::new(1280, 800).expect("the pixmap is made");
            render(&mut pixmap, look);
            let path = std::env::temp_dir().join(name);
            pixmap.save_png(&path).expect("the picture is written");
            println!("{}", path.display());
        }
    }

    #[test]
    fn one_frame_draws_into_a_picture() {
        let mut pixmap = tiny_skia::Pixmap::new(400, 300).expect("the pixmap is made");
        render(&mut pixmap, Look::DARK);
        // Something was drawn: the background alone would leave the middle
        // blank, and the table's rows are lighter than the window fill.
        let distinct: std::collections::HashSet<[u8; 4]> = pixmap
            .pixels()
            .iter()
            .map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        assert!(
            distinct.len() > 8,
            "the frame drew {} colours",
            distinct.len()
        );
    }
}
