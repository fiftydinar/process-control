//! Drawing: the graphs, the process table and the details panel.
//!
//! Every function here reads [`App`] state and turns input into calls on it.

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, TextStyle, vec2,
};

use crate::app::App;
use crate::apps::{SortKey, ViewMode};
use crate::config::REFRESH_CHOICES;
use crate::format;
use crate::sample::Signal;
use crate::theme::{self, Theme};

/// The colours the window is drawing with.
///
/// They are read from the style in use rather than from the theme the app
/// remembers, so what a row paints cannot disagree with what egui drew around
/// it. A theme switch writes the style and the remembered theme together, but
/// only one of them is what the frame is actually rendered with.
fn palette(ui: &egui::Ui) -> theme::Palette {
    use egui::Visuals;
    let visuals: &Visuals = ui.visuals();
    let rgb = |color: Color32| {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        (r, g, b)
    };
    theme::Palette {
        background: rgb(visuals.panel_fill),
        panel: rgb(visuals.window_fill),
        recess: rgb(visuals.extreme_bg_color),
        placeholder: rgb(visuals.faint_bg_color),
        widget: rgb(visuals.widgets.inactive.bg_fill),
        hovered: rgb(visuals.widgets.hovered.bg_fill),
        active: rgb(visuals.widgets.active.bg_fill),
        border: rgb(visuals.window_stroke.color),
        text: visuals.text_color(),
        weak_text: visuals.weak_text_color(),
        accent: visuals.hyperlink_color,
        light: !visuals.dark_mode,
    }
}

/// Height of one row of the process table, in points.
const ROW_HEIGHT: f32 = 22.0;

/// Height of the column titles above the table, in points.
const HEADER_HEIGHT: f32 = 22.0;

/// Height of the strip of graphs across the top, in points.
const GRAPH_HEIGHT: f32 = 96.0;

/// Height of the tool bar below the menu bar, in points.
const TOOLBAR_HEIGHT: f32 = 34.0;

/// Width of the details panel, in points.
const DETAILS_WIDTH: f32 = 380.0;

/// Width of the row's application icon column, in points.
const ICON_COLUMN: f32 = 22.0;

/// How far a row steps in for each level of the tree, in points.
const INDENT: f32 = 14.0;

/// The side of the arrow that folds a row away, in points.
const ARROW: f32 = 11.0;

/// Margin inside a table row, in points.
const ROW_MARGIN: f32 = 6.0;

/// How much of a bar's width the value fills, drawn as a faint track behind
/// the number.
const BAR_ALPHA: u8 = 46;

/// The columns of the process table, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
    /// The application's icon.
    Icon,
    /// The name, which the tree indents.
    Name,
    /// The process id.
    Pid,
    /// The name of the user that owns the process.
    User,
    /// CPU usage.
    Cpu,
    /// Memory held.
    Memory,
    /// Storage traffic.
    Disk,
    /// Network traffic.
    Network,
    /// GPU usage.
    Gpu,
    /// How many threads.
    Threads,
    /// The single letter state.
    State,
}

impl Column {
    /// Every column, left to right.
    pub(crate) const ALL: [Column; 11] = [
        Column::Icon,
        Column::Name,
        Column::Pid,
        Column::User,
        Column::Cpu,
        Column::Memory,
        Column::Disk,
        Column::Network,
        Column::Gpu,
        Column::Threads,
        Column::State,
    ];

    /// The name the column title shows.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Column::Icon => "",
            Column::Name => "Name",
            Column::Pid => "PID",
            Column::User => "User",
            Column::Cpu => "CPU",
            Column::Memory => "Memory",
            Column::Disk => "Disk",
            Column::Network => "Network",
            Column::Gpu => "GPU",
            Column::Threads => "Threads",
            Column::State => "State",
        }
    }

    /// The width the column takes, or `None` for the name, which takes what
    /// is left.
    fn width(self) -> Option<f32> {
        match self {
            Column::Icon => Some(ICON_COLUMN),
            Column::Name => None,
            Column::Pid => Some(58.0),
            Column::User => Some(78.0),
            Column::Cpu => Some(74.0),
            Column::Memory => Some(82.0),
            Column::Disk => Some(74.0),
            Column::Network => Some(78.0),
            Column::Gpu => Some(60.0),
            Column::Threads => Some(56.0),
            Column::State => Some(52.0),
        }
    }

    /// The sort key the column title asks for, when it has one.
    fn sort_key(self) -> Option<SortKey> {
        match self {
            Column::Name => Some(SortKey::Name),
            Column::Pid => Some(SortKey::Pid),
            Column::Cpu => Some(SortKey::Cpu),
            Column::Memory => Some(SortKey::Memory),
            Column::Disk => Some(SortKey::Disk),
            Column::Network => Some(SortKey::Network),
            Column::Gpu => Some(SortKey::Gpu),
            Column::Threads => Some(SortKey::Processes),
            _ => None,
        }
    }

    /// Whether the column holds a number, which is drawn at the right edge.
    fn numeric(self) -> bool {
        matches!(
            self,
            Column::Pid
                | Column::Cpu
                | Column::Memory
                | Column::Disk
                | Column::Network
                | Column::Gpu
                | Column::Threads
        )
    }
}

/// Which columns are shown, which a narrow window trims.
///
/// A window with no room for everything shows what a task manager is opened
/// for: the name, the CPU and the memory. The rest come back as it grows, and
/// they keep their places in [`Column::ALL`] so a column does not move as the
/// window is resized.
fn columns_for(width: f32) -> Vec<Column> {
    let mut shown = vec![
        Column::Icon,
        Column::Name,
        Column::Pid,
        Column::Cpu,
        Column::Memory,
    ];
    if width > 760.0 {
        shown.extend([Column::Disk, Column::Network]);
    }
    if width > 900.0 {
        shown.push(Column::Gpu);
    }
    if width > 1_000.0 {
        shown.push(Column::User);
    }
    if width > 1_080.0 {
        shown.extend([Column::Threads, Column::State]);
    }
    Column::ALL
        .into_iter()
        .filter(|column| shown.contains(column))
        .collect()
}

/// A column's rectangle, given the table's width and the space its numbers
/// were measured at.
fn layout(width: f32, columns: &[Column]) -> Vec<(Column, Rect)> {
    let fixed: f32 = columns.iter().filter_map(|column| column.width()).sum();
    let name = (width - fixed).max(120.0);
    let mut x = 0.0;
    let mut placed = Vec::with_capacity(columns.len());
    for column in columns {
        let column_width = column.width().unwrap_or(name);
        placed.push((
            *column,
            Rect::from_min_size(egui::pos2(x, 0.0), vec2(column_width, ROW_HEIGHT)),
        ));
        x += column_width;
    }
    placed
}

/// Draws the whole window.
pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.rebuild();

    if app.icons.take(&ctx) {
        ctx.request_repaint();
    }

    menu_bar(app, ui);
    tool_bar(app, ui);
    status_bar(app, ui);
    graphs(app, ui);

    // The details panel is drawn before the table so the table's width is
    // what is left of the window rather than the whole of it.
    if app.details.is_some() {
        egui::Panel::right("details")
            .exact_size(DETAILS_WIDTH)
            .show(ui, |ui| details_panel(app, ui));
    }

    // The frame carries the window's own colour: a frame with no fill leaves
    // the table drawing over whatever the renderer cleared to, which is a dark
    // grey, and the light themes then showed a dark column of rows.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
        .show(ui, |ui| table(app, ui));

    confirmation(app, &ctx);
}

/// The File, View and Help menus.
fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Refresh Now").clicked() {
                    ui.close();
                    app.sampler.refresh();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("View", |ui| {
                for mode in [ViewMode::Applications, ViewMode::Tree] {
                    if ui.selectable_label(app.view == mode, mode.name()).clicked() {
                        app.view = mode;
                        app.save();
                        ui.close();
                    }
                }
                ui.separator();
                let mut kernel = app.config.show_kernel_threads.unwrap_or(false);
                if ui
                    .checkbox(&mut kernel, "Show Kernel Threads")
                    .on_hover_text(
                        "The kernel's own threads, which hold no memory and cannot be stopped",
                    )
                    .changed()
                {
                    app.config.show_kernel_threads = Some(kernel);
                    app.save();
                }
                let mut icons = app.show_icons;
                if ui.checkbox(&mut icons, "Show Application Icons").changed() {
                    app.show_icons = icons;
                    app.save();
                }
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    for theme in Theme::ALL {
                        if ui
                            .selectable_label(app.theme == theme, theme.name())
                            .clicked()
                        {
                            app.set_theme(ui.ctx(), theme);
                            ui.ctx().request_repaint();
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Refresh Every", |ui| {
                    for (milliseconds, name) in REFRESH_CHOICES {
                        if ui
                            .selectable_label(app.config.refresh() == milliseconds, name)
                            .clicked()
                        {
                            app.set_refresh(milliseconds);
                            ui.close();
                        }
                    }
                });
            });

            ui.menu_button("Selection", |ui| {
                for signal in Signal::ALL {
                    let label = if signal == Signal::Kill {
                        "Force Stop"
                    } else {
                        signal.name()
                    };
                    if ui
                        .add_enabled(!app.selection.is_empty(), egui::Button::new(label))
                        .clicked()
                    {
                        ui.close();
                        app.send_signal(signal);
                    }
                }
            });

            ui.menu_button("Help", |ui| {
                ui.label(format!("{} {}", App::TITLE, env!("CARGO_PKG_VERSION")));
                ui.label("Reads the process table itself, and starts at once.");
                ui.separator();
                ui.label(format!("{} desktop entries", app.entries.len()));
                match crate::platform::gpu_name() {
                    Some(gpu) => ui.label(format!("GPU driver: {gpu}")),
                    None => ui.label(RichText::new("No GPU reported").weak()),
                };
                ui.hyperlink_to(
                    "Documentation",
                    "https://pkgforge-apps.github.io/process-control/",
                );
                ui.label("MIT licensed.");
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The rate the list is read at, which the menu also sets; it
                // is shown here because it explains why a number is moving.
                ui.label(
                    RichText::new(format!("{} ms", app.config.refresh()))
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
    });
}

/// The tool bar: the filter, the view switch and the actions.
fn tool_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("toolbar")
        .exact_size(TOOLBAR_HEIGHT)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.filter)
                        .desired_width(190.0)
                        .hint_text("Filter by name or pid"),
                );
                if search.changed() {
                    // The selection may now point at a row that is hidden,
                    // which acts on the same processes either way.
                }
                if !app.filter.is_empty() && ui.small_button("Clear").clicked() {
                    app.filter.clear();
                }

                ui.separator();
                for mode in [ViewMode::Applications, ViewMode::Tree] {
                    if ui.selectable_label(app.view == mode, mode.name()).clicked() {
                        app.view = mode;
                        app.save();
                    }
                }

                ui.separator();
                let selection = app.selection.len();
                let label = if selection > 1 {
                    format!("Force Stop {selection}")
                } else {
                    "Force Stop".to_string()
                };
                if ui
                    .add_enabled(selection > 0, egui::Button::new(label))
                    .on_hover_text("SIGKILL: stop without letting the process clean up")
                    .clicked()
                {
                    app.send_signal(Signal::Kill);
                }
                if ui
                    .add_enabled(selection > 0, egui::Button::new("Terminate"))
                    .on_hover_text("SIGTERM: ask the process to stop")
                    .clicked()
                {
                    app.send_signal(Signal::Term);
                }
                ui.menu_button("Signal", |ui| {
                    for signal in Signal::ALL {
                        if ui
                            .add_enabled(selection > 0, egui::Button::new(signal.name()))
                            .clicked()
                        {
                            ui.close();
                            app.send_signal(signal);
                        }
                    }
                });
                if ui
                    .add_enabled(selection > 0, egui::Button::new("Details"))
                    .clicked()
                    && let Some(position) = app.rows.iter().position(|row| {
                        row.pids
                            .first()
                            .is_some_and(|pid| app.selection.contains(pid))
                    })
                {
                    app.open_details(position);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(app.process_summary()).color(ui.visuals().weak_text_color()),
                    );
                });
            });
        });
}

/// The status line, shown only when there is something to say.
fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    if let Some(error) = app.sample.error.clone() {
        egui::Panel::bottom("error").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
            });
        });
    }
    let Some(status) = app.status.clone() else {
        return;
    };
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(status).color(ui.visuals().weak_text_color()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Dismiss").clicked() {
                    app.status = None;
                }
            });
        });
    });
}

/// The strip of graphs across the top: CPU, memory, GPU, network and storage.
fn graphs(app: &App, ui: &mut egui::Ui) {
    egui::Panel::top("graphs")
        .exact_size(GRAPH_HEIGHT + 12.0)
        .show(ui, |ui| {
            let width = ui.available_width();
            let spacing = 8.0;
            let count = 5.0;
            let card = ((width - spacing * (count - 1.0)) / count).max(80.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                cpu_card(app, ui, card);
                memory_card(app, ui, card);
                gpu_card(app, ui, card);
                network_card(app, ui, card);
                disk_card(app, ui, card);
            });
        });
}

/// One graph card's history: one or two lines and the scale they share.
pub(crate) struct History<'a> {
    /// The first line, drawn brighter.
    pub first: &'a [f32],
    /// A second line, drawn behind the first, if there is one.
    pub second: &'a [f32],
    /// The value the top of the plot stands for.
    pub ceiling: f32,
}

/// One graph card: a title, a large reading and a history.
fn card(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    value: String,
    detail: String,
    history: History<'_>,
    color: Color32,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, GRAPH_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    painter.rect_filled(rect, 6.0, visuals.faint_bg_color);

    let inner = rect.shrink2(vec2(9.0, 6.0));
    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        title.to_uppercase(),
        FontId::proportional(9.5),
        visuals.weak_text_color(),
    );

    // The reading and the line under it are separate rows, and neither may
    // run past the card: a long reading used to be drawn from the left while
    // the detail was drawn from the right, on the same line, so they ran into
    // each other. Each is clipped to the card instead.
    let value_rect = Rect::from_min_max(
        egui::pos2(inner.left(), inner.top() + 12.0),
        egui::pos2(inner.right(), inner.top() + 32.0),
    );
    painter.text(
        value_rect.left_center(),
        Align2::LEFT_CENTER,
        truncate(ui, &value, 19.0, value_rect.width()),
        FontId::proportional(19.0),
        visuals.strong_text_color(),
    );
    let detail_rect = Rect::from_min_max(
        egui::pos2(inner.left(), inner.top() + 33.0),
        egui::pos2(inner.right(), inner.top() + 45.0),
    );
    painter.text(
        detail_rect.left_center(),
        Align2::LEFT_CENTER,
        truncate(ui, &detail, 10.0, detail_rect.width()),
        FontId::proportional(10.0),
        visuals.weak_text_color(),
    );

    let plot = Rect::from_min_max(
        egui::pos2(inner.left(), inner.top() + 48.0),
        egui::pos2(inner.right(), rect.bottom() - 6.0),
    );
    graph(&painter, plot, history.first, history.ceiling, color);
    if !history.second.is_empty() {
        graph(
            &painter,
            plot,
            history.second,
            history.ceiling,
            color.gamma_multiply(0.55),
        );
    }
    response.on_hover_text(format!("{title}\n{value}\n{detail}"));
}

/// `text` cut to what fits in `width` at `size`, with an ellipsis when it does
/// not, so a long reading is shortened rather than drawn over its neighbour.
fn truncate(ui: &egui::Ui, text: &str, size: f32, width: f32) -> String {
    let font = FontId::proportional(size);
    let fits = |candidate: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(candidate.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        }) <= width
    };
    if fits(text) {
        return text.to_string();
    }
    let mut cut: String = String::new();
    for letter in text.chars() {
        let mut candidate = cut.clone();
        candidate.push(letter);
        candidate.push('\u{2026}');
        if !fits(&candidate) {
            break;
        }
        cut.push(letter);
    }
    cut.push('\u{2026}');
    cut
}

/// Draws one history as a filled line across `plot`.
fn graph(painter: &egui::Painter, plot: Rect, history: &[f32], ceiling: f32, color: Color32) {
    if history.len() < 2 || plot.width() < 2.0 || plot.height() < 2.0 {
        return;
    }
    let ceiling = ceiling.max(0.0001);
    let step = plot.width() / (crate::app::HISTORY - 1) as f32;
    let origin = plot.right() - step * (history.len() - 1) as f32;
    let point = |index: usize, value: f32| {
        let x = origin + index as f32 * step;
        let y = plot.bottom() - (value / ceiling).clamp(0.0, 1.0) * plot.height();
        egui::pos2(x, y)
    };
    let points: Vec<egui::Pos2> = history
        .iter()
        .enumerate()
        .map(|(index, value)| point(index, *value))
        .collect();

    let mut fill = points.clone();
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        fill.push(egui::pos2(last.x, plot.bottom()));
        fill.push(egui::pos2(first.x, plot.bottom()));
    }
    painter.add(egui::Shape::convex_polygon(
        fill,
        color.gamma_multiply(0.20),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::line(points, Stroke::new(1.4, color)));
}

/// The CPU graph, with the busiest core's share beside the total.
fn cpu_card(app: &App, ui: &mut egui::Ui, width: f32) {
    let sample = &app.sample;
    let palette = palette(ui);
    let busiest = sample.cores.iter().copied().fold(0.0_f32, f32::max);
    card(
        ui,
        width,
        "CPU",
        format::percent(sample.cpu),
        format!("{} cores, busiest {:.0}%", sample.cpu_count, busiest),
        History {
            first: &app.cpu_history,
            second: &[],
            ceiling: 100.0,
        },
        palette.accent,
    );
}

/// The memory graph, with the swap beside the total.
fn memory_card(app: &App, ui: &mut egui::Ui, width: f32) {
    let sample = &app.sample;
    let palette = palette(ui);
    let detail = if sample.swap_total > 0 {
        format!(
            "{} of {}, swap {}",
            format::percent(sample.memory_fraction() * 100.0),
            format::bytes(sample.memory_total),
            format::percent(sample.swap_fraction() * 100.0)
        )
    } else {
        format!(
            "{} of {}",
            format::percent(sample.memory_fraction() * 100.0),
            format::bytes(sample.memory_total)
        )
    };
    card(
        ui,
        width,
        "Memory",
        format!(
            "{} / {}",
            format::bytes(sample.memory_used),
            format::bytes(sample.memory_total)
        ),
        detail,
        History {
            first: &app.memory_history,
            second: &[],
            ceiling: 100.0,
        },
        palette.accent.gamma_multiply(0.85),
    );
}

/// The GPU graph, which says so when the machine reports no GPU.
fn gpu_card(app: &App, ui: &mut egui::Ui, width: f32) {
    let sample = &app.sample;
    let palette = palette(ui);
    match sample.gpu {
        Some(busy) => card(
            ui,
            width,
            "GPU",
            format::percent(busy),
            format::bytes(sample.gpu_memory),
            History {
                first: &app.gpu_history,
                second: &[],
                ceiling: 100.0,
            },
            palette.accent.gamma_multiply(0.7),
        ),
        None => card(
            ui,
            width,
            "GPU",
            "--".to_string(),
            "not reported".to_string(),
            History {
                first: &[],
                second: &[],
                ceiling: 100.0,
            },
            palette.weak_text,
        ),
    }
}

/// The network graph, receiving over sending.
fn network_card(app: &App, ui: &mut egui::Ui, width: f32) {
    let sample = &app.sample;
    let palette = palette(ui);
    let ceiling = app.scales.net.max(1.0);
    card(
        ui,
        width,
        "Network",
        format::rate(sample.net_read + sample.net_write),
        format!(
            "{} down, {} up",
            format::rate(sample.net_read),
            format::rate(sample.net_write)
        ),
        History {
            first: &app.net_read_history,
            second: &app.net_write_history,
            ceiling,
        },
        palette.accent.gamma_multiply(0.8),
    );
}

/// The storage graph, reading over writing.
fn disk_card(app: &App, ui: &mut egui::Ui, width: f32) {
    let sample = &app.sample;
    let palette = palette(ui);
    let ceiling = app.scales.disk.max(1.0);
    card(
        ui,
        width,
        "Storage",
        format::rate(sample.disk_read + sample.disk_write),
        format!(
            "{} read, {} write",
            format::rate(sample.disk_read),
            format::rate(sample.disk_write)
        ),
        History {
            first: &app.disk_read_history,
            second: &app.disk_write_history,
            ceiling,
        },
        palette.accent.gamma_multiply(0.65),
    );
}

/// The process table: column titles, then the rows.
fn table(app: &mut App, ui: &mut egui::Ui) {
    let visible = app.visible_rows();
    let columns = columns_for(ui.available_width());
    if !app.show_icons {
        let columns: Vec<Column> = columns
            .into_iter()
            .filter(|column| *column != Column::Icon)
            .collect();
        return table_with(app, ui, &visible, &columns);
    }
    table_with(app, ui, &visible, &columns);
}

/// Draws the table with a fixed set of columns.
fn table_with(app: &mut App, ui: &mut egui::Ui, visible: &[usize], columns: &[Column]) {
    // The scroll bar takes its width out of the panel, so a table that used
    // all of it would have its last column under the bar.
    let bar = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
    let width = (ui.available_width() - bar).max(200.0);
    let places = layout(width, columns);
    // The titles stay put while the rows scroll, so they are drawn first and
    // the rows are clipped below them.
    let header = Rect::from_min_size(ui.cursor().min, vec2(width, HEADER_HEIGHT));
    ui.add_space(HEADER_HEIGHT);
    let clip = ui.clip_rect();
    ui.set_clip_rect(Rect::from_min_max(
        egui::pos2(clip.min.x, header.bottom()),
        clip.max,
    ));

    let mut clicked: Option<(usize, egui::PointerButton)> = None;
    // A table's rows sit right against each other: the row height already
    // holds the gap a widget would want.
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, visible.len(), |ui, range| {
            for position in range {
                let row_index = visible[position];
                let heading = app
                    .rows
                    .get(row_index)
                    .is_some_and(crate::apps::Row::is_heading);
                // A heading names a section rather than standing for a
                // process, so it takes no click and opens no menu.
                let sense = if heading {
                    Sense::hover()
                } else {
                    Sense::click()
                };
                let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), sense);
                if rect.bottom() < ui.clip_rect().top() || rect.top() > ui.clip_rect().bottom() {
                    continue;
                }
                row(app, ui, row_index, rect, &places, &response);
                // The row is painted rather than built from widgets, so its
                // text is announced here: a screen reader would otherwise see
                // a table of nothing.
                if let Some(entry) = app.rows.get(row_index) {
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Label, true, row_description(entry))
                    });
                }
                if heading {
                    continue;
                }
                if response.clicked() {
                    clicked = Some((row_index, egui::PointerButton::Primary));
                } else if response.secondary_clicked() {
                    clicked = Some((row_index, egui::PointerButton::Secondary));
                }
                if response.double_clicked() {
                    app.open_details(row_index);
                }
                response.context_menu(|ui| row_menu(app, ui, row_index));
            }
        });
    ui.set_clip_rect(clip);
    column_titles(app, ui, header, &places);
    if let Some((row_index, button)) = clicked {
        match button {
            egui::PointerButton::Primary => {
                let (shift, command) =
                    ui.input(|input| (input.modifiers.shift, input.modifiers.command));
                if shift {
                    app.extend_to(row_index);
                } else if command {
                    app.toggle_row(row_index);
                } else {
                    app.select_row(row_index);
                }
            }
            egui::PointerButton::Secondary
                if !app.rows[row_index]
                    .pids
                    .first()
                    .is_some_and(|pid| app.selection.contains(pid)) =>
            {
                app.select_row(row_index);
            }
            _ => {}
        }
    }

    if visible.is_empty() {
        ui.centered_and_justified(|ui| {
            let hint = if app.sample.error.is_some() {
                "Nothing is being read on this system"
            } else if !app.filter.trim().is_empty() {
                "Nothing matches that filter"
            } else {
                "Reading the process table..."
            };
            ui.label(RichText::new(hint).color(ui.visuals().weak_text_color()));
        });
    }
}

/// Draws the column titles, which a click sorts by.
fn column_titles(app: &mut App, ui: &mut egui::Ui, header: Rect, places: &[(Column, Rect)]) {
    let painter = ui.painter_at(header);
    let palette = palette(ui);
    painter.rect_filled(header, 0.0, ui.visuals().faint_bg_color);
    let mut sort_click: Option<SortKey> = None;
    for (column, place) in places {
        let rect = Rect::from_min_size(
            egui::pos2(header.left() + place.left(), header.top()),
            vec2(place.width(), HEADER_HEIGHT),
        );
        let key = column.sort_key();
        let active = key.is_some_and(|key| key == app.sort);
        let response = ui.interact(
            rect,
            ui.scope_id().with(("column", column.label())),
            if key.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let color = if active {
            palette.accent
        } else {
            palette.weak_text
        };
        let mut label = column.label().to_string();
        if active {
            label.push_str(if app.descending { " v" } else { " ^" });
        }
        let anchor = if column.numeric() {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let position = if column.numeric() {
            egui::pos2(rect.right() - ROW_MARGIN, rect.center().y)
        } else {
            egui::pos2(rect.left() + ROW_MARGIN, rect.center().y)
        };
        painter.text(position, anchor, label, FontId::proportional(11.0), color);
        if response.clicked()
            && let Some(key) = key
        {
            sort_click = Some(key);
        }
    }
    // Sorting changes the rows, which cannot happen while the titles are
    // drawn from a borrow of them.
    if let Some(key) = sort_click {
        app.sort_by(key);
    }
}

/// Draws one row of the table.
fn row(
    app: &mut App,
    ui: &mut egui::Ui,
    position: usize,
    rect: Rect,
    places: &[(Column, Rect)],
    response: &egui::Response,
) {
    let Some(entry) = app.rows.get(position).cloned() else {
        return;
    };
    let palette = palette(ui);
    let painter = ui.painter_at(rect);

    // A heading is a band across the whole table that names the section under
    // it and adds up what that section holds. It stands for no process, so it
    // takes no selection, but it folds like a row.
    if entry.is_heading() {
        painter.rect_filled(rect, 0.0, ui.visuals().widgets.noninteractive.bg_fill);
        let arrow = Rect::from_center_size(
            egui::pos2(rect.left() + ROW_MARGIN + ARROW / 2.0, rect.center().y),
            vec2(ARROW, ARROW),
        );
        let points = if entry.folded {
            vec![
                egui::pos2(arrow.left() + 3.0, arrow.top() + 2.0),
                egui::pos2(arrow.right() - 2.0, arrow.center().y),
                egui::pos2(arrow.left() + 3.0, arrow.bottom() - 2.0),
            ]
        } else {
            vec![
                egui::pos2(arrow.left() + 2.0, arrow.top() + 3.0),
                egui::pos2(arrow.right() - 2.0, arrow.top() + 3.0),
                egui::pos2(arrow.center().x, arrow.bottom() - 2.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            palette.weak_text,
            Stroke::NONE,
        ));
        painter.text(
            egui::pos2(arrow.right() + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            entry.name.to_uppercase(),
            FontId::proportional(10.0),
            palette.weak_text,
        );
        // The section's own totals, in the columns they belong to.
        for (column, place) in places {
            let cell = Rect::from_min_size(
                egui::pos2(rect.left() + place.left(), rect.top()),
                vec2(place.width(), rect.height()),
            );
            let text = match column {
                Column::Cpu => format::percent(entry.totals.cpu),
                Column::Memory => format::bytes(entry.totals.memory),
                Column::Disk => format::rate(entry.totals.disk_read + entry.totals.disk_write),
                Column::Network => format::rate(entry.totals.net_read + entry.totals.net_write),
                Column::Gpu if entry.totals.gpu > 0.0 => format::percent(entry.totals.gpu),
                Column::Threads => entry.totals.count.to_string(),
                _ => continue,
            };
            painter.text(
                egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                Align2::RIGHT_CENTER,
                text,
                FontId::proportional(11.0),
                palette.weak_text,
            );
        }
        if ui
            .interact(
                arrow,
                ui.scope_id().with(("section", entry.name.as_str())),
                Sense::click(),
            )
            .clicked()
        {
            app.toggle_section(entry.section.expect("a heading names its section"));
        }
        return;
    }

    let selected = entry
        .pids
        .first()
        .is_some_and(|pid| app.selection.contains(pid));

    if selected {
        painter.rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }

    // Everything the row draws is copied out of the sample first: an icon
    // asked for below makes the icon set borrow itself mutably, and the
    // sample is borrowed again for the numbers.
    let pid = entry.pids.first().copied().unwrap_or_default();
    let process = app.sample.process(pid).cloned();
    let memory_total = app.sample.memory_total;
    let gpu_reported = app.sample.gpu.is_some();

    let texture = entry
        .icon
        .as_deref()
        .and_then(|icon| app.icons.texture(ui.ctx(), icon));
    let mut toggle = false;
    for (column, place) in places {
        let cell = Rect::from_min_size(
            egui::pos2(rect.left() + place.left(), rect.top()),
            vec2(place.width(), rect.height()),
        );
        match column {
            Column::Icon => {
                let square = Rect::from_center_size(
                    cell.center(),
                    vec2(crate::app::ICON_SIZE, crate::app::ICON_SIZE),
                );
                match &texture {
                    Some(texture) => {
                        painter.image(
                            texture.id(),
                            square,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        let letter = entry.name.chars().next().unwrap_or('\u{2022}');
                        // A placeholder is an icon stand-in, so it is drawn
                        // in the accent at low weight rather than as a black
                        // square that reads as a missing image.
                        crate::icons::Placeholder::paint(
                            &painter,
                            square,
                            letter,
                            palette.accent,
                            egui::Color32::from_rgb(
                                palette.background.0,
                                palette.background.1,
                                palette.background.2,
                            ),
                        );
                    }
                }
            }
            Column::Name => {
                let indent = entry.depth as f32 * INDENT;
                let arrow = Rect::from_center_size(
                    egui::pos2(
                        cell.left() + ROW_MARGIN + indent + ARROW / 2.0,
                        cell.center().y,
                    ),
                    vec2(ARROW, ARROW),
                );
                if entry.has_children {
                    let points = if entry.folded {
                        vec![
                            egui::pos2(arrow.left() + 3.0, arrow.top() + 2.0),
                            egui::pos2(arrow.right() - 2.0, arrow.center().y),
                            egui::pos2(arrow.left() + 3.0, arrow.bottom() - 2.0),
                        ]
                    } else {
                        vec![
                            egui::pos2(arrow.left() + 2.0, arrow.top() + 3.0),
                            egui::pos2(arrow.right() - 2.0, arrow.top() + 3.0),
                            egui::pos2(arrow.center().x, arrow.bottom() - 2.0),
                        ]
                    };
                    painter.add(egui::Shape::convex_polygon(
                        points,
                        palette.weak_text,
                        Stroke::NONE,
                    ));
                    if ui
                        .interact(arrow, ui.scope_id().with(("fold", pid)), Sense::click())
                        .clicked()
                    {
                        toggle = true;
                    }
                }
                let text_left = cell.left() + ROW_MARGIN + indent + ARROW + 4.0;
                // An application's own row is the one that names the program;
                // its processes are drawn a shade quieter so the tree reads as
                // one program with parts rather than as a list of equals.
                let color = if !entry.is_user {
                    palette.weak_text
                } else if entry.is_application {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals().text_color()
                };
                let galley = ui.painter().layout(
                    entry.name.clone(),
                    FontId::proportional(12.5),
                    color,
                    cell.right() - text_left - ROW_MARGIN,
                );
                painter.galley(
                    egui::pos2(text_left, cell.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
            }
            Column::Pid => {
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    entry.pids.first().copied().unwrap_or_default().to_string(),
                    FontId::monospace(11.0),
                    palette.weak_text,
                );
            }
            Column::User => {
                let user = user_name(process.as_ref().map(|process| process.uid).unwrap_or(0));
                painter.text(
                    egui::pos2(cell.left() + ROW_MARGIN, cell.center().y),
                    Align2::LEFT_CENTER,
                    user,
                    FontId::proportional(11.5),
                    palette.weak_text,
                );
            }
            Column::Cpu => {
                bar(
                    &painter,
                    cell,
                    entry.totals.cpu / 100.0,
                    format::percent(entry.totals.cpu),
                    palette.accent,
                    palette,
                );
            }
            Column::Memory => {
                let fraction = crate::format::fraction(entry.totals.memory, memory_total);
                bar(
                    &painter,
                    cell,
                    fraction,
                    format::bytes(entry.totals.memory),
                    palette.accent.gamma_multiply(0.85),
                    palette,
                );
            }
            Column::Disk => {
                let rate = entry.totals.disk_read + entry.totals.disk_write;
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    format::rate(rate),
                    FontId::proportional(11.5),
                    palette.weak_text,
                );
            }
            Column::Network => {
                let rate = entry.totals.net_read + entry.totals.net_write;
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    format::rate(rate),
                    FontId::proportional(11.5),
                    palette.weak_text,
                );
            }
            Column::Gpu => {
                let text = if !gpu_reported || entry.totals.gpu <= 0.0 {
                    "-".to_string()
                } else {
                    format::percent(entry.totals.gpu)
                };
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    text,
                    FontId::proportional(11.5),
                    palette.weak_text,
                );
            }
            Column::Threads => {
                let threads = process.as_ref().map(|process| process.threads).unwrap_or(0);
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    threads.to_string(),
                    FontId::monospace(11.0),
                    palette.weak_text,
                );
            }
            Column::State => {
                let state = process.as_ref().map(|process| process.state).unwrap_or('-');
                painter.text(
                    egui::pos2(cell.right() - ROW_MARGIN, cell.center().y),
                    Align2::RIGHT_CENTER,
                    state.to_string(),
                    FontId::monospace(11.0),
                    palette.weak_text,
                );
            }
        }
    }
    if toggle {
        app.toggle_fold(pid);
    }
    // The hover text carries what the row has no column for.
    if response.hovered()
        && let Some(process) = &process
    {
        let mut text = String::new();
        if let Some(exe) = &process.exe {
            text.push_str(exe);
            text.push('\n');
        }
        if let Some(uptime) = process.uptime() {
            text.push_str(&format!("running for {}\n", format::duration(uptime)));
        }
        if process.threads > 1 {
            text.push_str(&format!("{} threads\n", process.threads));
        }
        if process.cpu_time > 0.0 {
            text.push_str(&format!(
                "{} CPU time",
                format::duration(process.cpu_time as u64)
            ));
        }
        if !text.is_empty() {
            response.clone().on_hover_text(text.trim_end());
        }
    }
}

/// Draws a number at the right edge of `cell` over a bar filling `fraction`.
fn bar(
    painter: &egui::Painter,
    cell: Rect,
    fraction: f32,
    text: String,
    color: Color32,
    palette: theme::Palette,
) {
    let track = Rect::from_min_max(
        egui::pos2(cell.left() + ROW_MARGIN, cell.center().y - 5.0),
        egui::pos2(cell.right() - ROW_MARGIN, cell.center().y + 5.0),
    );
    let filled = Rect::from_min_max(
        track.min,
        egui::pos2(
            track.left() + track.width() * fraction.clamp(0.0, 1.0),
            track.bottom(),
        ),
    );
    painter.rect_filled(track, 2.0, color.gamma_multiply(0.10));
    if filled.width() > 0.5 {
        painter.rect_filled(filled, 2.0, color.gamma_multiply(BAR_ALPHA as f32 / 255.0));
    }
    painter.text(
        egui::pos2(track.right() - 2.0, track.center().y),
        Align2::RIGHT_CENTER,
        text,
        FontId::proportional(11.5),
        palette.text,
    );
}

/// What a row says about itself, which a screen reader reads out.
fn row_description(row: &crate::apps::Row) -> String {
    format!(
        "{}: {} CPU, {}",
        row.name,
        format::percent(row.totals.cpu),
        format::bytes(row.totals.memory)
    )
}

/// The menu a right click on a row opens.
fn row_menu(app: &mut App, ui: &mut egui::Ui, position: usize) {
    let name = app
        .rows
        .get(position)
        .map(|row| row.name.clone())
        .unwrap_or_default();
    ui.label(RichText::new(name).color(ui.visuals().weak_text_color()));
    ui.separator();
    if ui.button("Details").clicked() {
        ui.close();
        app.open_details(position);
    }
    ui.separator();
    for signal in Signal::ALL {
        let label = if signal == Signal::Kill {
            "Force Stop".to_string()
        } else {
            signal.name().to_string()
        };
        let response = ui.add_enabled(
            app.rows.get(position).is_some_and(|row| {
                row.pids
                    .first()
                    .is_some_and(|pid| app.selection.contains(pid))
            }),
            egui::Button::new(label),
        );
        if response.clicked() {
            ui.close();
            app.send_signal(signal);
        }
    }
}

/// The panel that shows one process closely.
fn details_panel(app: &mut App, ui: &mut egui::Ui) {
    let Some(details) = app.details.as_ref() else {
        return;
    };
    let info = details.info.clone();
    let wm_class = details.wm_class.clone();
    let read = details.read;
    let uptime = crate::platform::uptime_of(&info);

    ui.horizontal(|ui| {
        ui.heading(info.name.clone());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Close").clicked() {
                app.details = None;
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("PID {}", info.pid)).color(ui.visuals().weak_text_color()));
        // The window class is what groups a program's processes into an
        // application, so it is worth reading at a glance rather than hunting
        // for in the grid.
        if let Some(wm_class) = &wm_class {
            ui.label(format!("class {wm_class}"))
                .on_hover_text("The window class a desktop entry names as StartupWMClass");
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Terminate").clicked()
            && let Err(error) = crate::platform::signal(info.pid, Signal::Term)
        {
            app.status = Some(error);
        }
        if ui.button("Force Stop").clicked() {
            app.confirmation = Some(crate::app::Confirmation {
                pids: vec![info.pid],
                name: info.name.clone(),
                signal: Signal::Kill,
            });
        }
        ui.menu_button("Signal", |ui| {
            for signal in Signal::ALL {
                if ui.button(signal.name()).clicked() {
                    ui.close();
                    if let Err(error) = crate::platform::signal(info.pid, signal) {
                        app.status = Some(error);
                    }
                }
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("details-grid")
                .num_columns(2)
                .spacing(vec2(12.0, 5.0))
                .show(ui, |ui| {
                    let mut field = |name: &str, value: String| {
                        ui.label(RichText::new(name).color(ui.visuals().weak_text_color()));
                        ui.label(value);
                        ui.end_row();
                    };
                    field("User", format!("{} ({})", info.user, info.uid));
                    field("State", state_name(info.state));
                    field("Parent", info.ppid.to_string());
                    field("Threads", info.threads.to_string());
                    field("Nice", info.nice.to_string());
                    field("CPU time", format::duration(info.cpu_time as u64));
                    field(
                        "Memory",
                        format!(
                            "{} of {}",
                            format::bytes(info.memory),
                            format::bytes(info.virtual_memory)
                        ),
                    );
                    if info.swap > 0 {
                        field("Swap", format::bytes(info.swap));
                    }
                    field(
                        "Storage",
                        format!(
                            "{} read, {} written",
                            format::bytes(info.io_read),
                            format::bytes(info.io_write)
                        ),
                    );
                    field("Open files", info.fds.to_string());
                    field("OOM score", info.oom_score.to_string());
                    if let Some(uptime) = uptime {
                        field("Running for", format::duration(uptime.as_secs()));
                    }
                });

            ui.add_space(8.0);
            section(ui, "Executable", info.exe.clone());
            section(
                ui,
                "Command",
                (!info.cmdline.is_empty()).then(|| info.cmdline.clone()),
            );
            section(ui, "Working directory", info.cwd.clone());
            if let Some(root) = &info.root
                && root != "/"
            {
                section(ui, "Root", Some(root.clone()));
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "read {} ago",
                    format::duration(read.elapsed().as_secs())
                ))
                .small()
                .color(ui.visuals().weak_text_color()),
            );
        });
}

/// One labelled, selectable paragraph of the details panel.
fn section(ui: &mut egui::Ui, title: &str, value: Option<String>) {
    let Some(value) = value else {
        return;
    };
    ui.label(RichText::new(title).color(ui.visuals().weak_text_color()));
    let mut text = value;
    // A command line is worth copying, so it is a text box the user can
    // select from rather than a label that cannot be.
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .desired_width(f32::INFINITY)
            .desired_rows(1)
            .font(TextStyle::Monospace)
            .interactive(true),
    );
    ui.add_space(6.0);
}

/// The question a force stop asks before it acts.
fn confirmation(app: &mut App, ctx: &egui::Context) {
    let Some(question) = app.confirmation.as_ref().map(|c| c.question()) else {
        return;
    };
    let action = app
        .confirmation
        .as_ref()
        .map(|c| c.action())
        .unwrap_or("Send");
    let signal = app
        .confirmation
        .as_ref()
        .map(|c| c.signal)
        .unwrap_or(Signal::Kill);

    let mut open = true;
    let response = egui::Modal::new(egui::Id::unique("confirmation")).show(ctx, |ui| {
        ui.set_max_width(340.0);
        ui.heading("Are you sure?");
        ui.add_space(4.0);
        ui.label(question);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(action).fill(ui.visuals().warn_fg_color))
                .clicked()
            {
                app.deliver(signal);
                open = false;
            }
            if ui.button("Cancel").clicked() {
                app.confirmation = None;
                open = false;
            }
        });
    });
    if response.should_close() {
        open = false;
    }
    if !open && app.confirmation.is_some() && response.should_close() {
        app.confirmation = None;
    }
}

/// A state letter as a word, which the details panel shows.
fn state_name(state: char) -> String {
    match state {
        'R' => "Running".to_string(),
        'S' => "Sleeping".to_string(),
        'D' => "Waiting on I/O".to_string(),
        'Z' => "Zombie".to_string(),
        'T' => "Stopped".to_string(),
        't' => "Tracing stop".to_string(),
        'X' | 'x' => "Dead".to_string(),
        'I' => "Idle".to_string(),
        'W' => "Paging".to_string(),
        'P' => "Parked".to_string(),
        other => other.to_string(),
    }
}

/// The name of the user with this id, which the table's user column shows.
fn user_name(uid: u32) -> String {
    crate::platform::user_name(uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// An app drawing the whole window, with a sample of its own rather than
    /// the one the reader thread sends.
    fn harness() -> Harness<'static, crate::app::App> {
        let ctx = egui::Context::default();
        let mut app = crate::app::App::new(&ctx, true);
        app.sample = crate::sample::Sample {
            processes: vec![
                crate::sample::ProcessInfo {
                    pid: 1,
                    name: "init".to_string(),
                    memory: 4096,
                    virtual_memory: 8192,
                    threads: 1,
                    state: 'S',
                    ..crate::sample::ProcessInfo::default()
                },
                crate::sample::ProcessInfo {
                    pid: 42,
                    ppid: 1,
                    name: "shell".to_string(),
                    memory: 2048,
                    virtual_memory: 4096,
                    threads: 3,
                    state: 'R',
                    cpu: 12.5,
                    ..crate::sample::ProcessInfo::default()
                },
            ],
            memory_total: 16 * 1024 * 1024 * 1024,
            memory_used: 4 * 1024 * 1024 * 1024,
            cpu_count: 8,
            process_count: 2,
            running: 1,
            ..crate::sample::Sample::default()
        };
        Harness::builder()
            .with_size(egui::vec2(1_040.0, 660.0))
            .build_ui_state(|ui, app: &mut crate::app::App| draw(app, ui), app)
    }

    #[test]
    fn the_window_draws_without_a_display_server() {
        let mut harness = harness();
        harness.run();
        for label in ["File", "View", "Selection", "Help"] {
            harness.get_by_label(label);
        }
    }

    #[test]
    fn the_tool_bar_offers_the_actions_a_task_manager_needs() {
        let mut harness = harness();
        harness.run();
        for label in ["Terminate", "Force Stop", "Signal", "Details"] {
            harness.get_by_label(label);
        }
    }

    #[test]
    fn the_table_lists_the_processes_of_the_sample() {
        let mut harness = harness();
        harness.run();
        // A row is painted rather than made of widgets, so what a screen
        // reader sees is the description the row gives of itself.
        let rows: Vec<String> = harness.state().rows.iter().map(row_description).collect();
        assert!(rows.iter().any(|row| row.starts_with("init: ")), "{rows:?}");
        assert!(
            rows.iter().any(|row| row.starts_with("shell: ")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("12.5% CPU")),
            "the numbers the row draws are in its description: {rows:?}"
        );
        // The application view lists a program that owns no window on its
        // own, at the top; the tree view puts it under its parent.
        harness.state_mut().view = crate::apps::ViewMode::Tree;
        harness.run();
        let shell = harness
            .state()
            .rows
            .iter()
            .find(|row| row.name == "shell")
            .expect("the shell is a row");
        assert_eq!(shell.depth, 1, "the shell sits under init in the tree");
    }

    #[test]
    fn a_reading_longer_than_its_card_is_cut_rather_than_drawn_over_its_detail() {
        // The memory card draws a reading and a detail line under it; a long
        // reading used to run under the detail, which is what made the two
        // overlap. A reading that does not fit is shortened with an ellipsis.
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1_040.0, 660.0))
            .build_ui_state(
                |ui, out: &mut (String, String)| {
                    out.0 = truncate(ui, "4.16 GiB / 5.62 GiB", 19.0, 60.0);
                    out.1 = truncate(ui, "12.3%", 10.0, 400.0);
                },
                (String::new(), String::new()),
            );
        harness.run();
        let (cut, fitted) = harness.state().clone();
        assert!(
            cut.ends_with('\u{2026}'),
            "a long reading is elided: {cut:?}"
        );
        assert!(cut.chars().count() < "4.16 GiB / 5.62 GiB".chars().count());
        assert_eq!(fitted, "12.3%", "a reading that fits is left alone");
    }

    #[test]
    fn the_columns_fit_what_the_window_has_room_for() {
        // A narrow window keeps the columns a task manager is opened for, and
        // a wide one adds the rest without reordering them.
        let narrow: Vec<Column> = columns_for(600.0);
        assert!(narrow.contains(&Column::Name));
        assert!(narrow.contains(&Column::Cpu));
        assert!(narrow.contains(&Column::Memory));
        assert!(!narrow.contains(&Column::User));

        let wide = columns_for(1_400.0);
        assert!(wide.len() > narrow.len());
        let order: Vec<Column> = Column::ALL
            .into_iter()
            .filter(|column| wide.contains(column))
            .collect();
        assert_eq!(wide, order, "the columns keep their places");
    }

    #[test]
    fn the_window_paints_a_real_amount_of_content() {
        // Draws the window into egui's own tessellator, which needs no GPU and
        // no display server. The graphs, the table and the tool bar are
        // hundreds of shapes; an empty window is a handful.
        let mut harness = harness();
        harness.run();
        let output = harness.output();
        let primitives = harness.ctx.tessellate(output.shapes.clone(), 1.0);
        let vertices: usize = primitives
            .iter()
            .map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                egui::epaint::Primitive::Callback(_) => 0,
            })
            .sum();
        assert!(
            vertices > 2_000,
            "the window draws its graphs and its table, not a blank screen: {vertices} vertices"
        );
    }

    #[test]
    fn a_row_shows_the_gpu_share_the_machine_reports() {
        let mut harness = harness();
        let mut sample = harness.state().sample.clone();
        sample.gpu = Some(30.0);
        sample.processes[1].gpu = 12.0;
        harness.state_mut().adopt_for_test(sample);
        harness.run();
        let row = harness
            .state()
            .rows
            .iter()
            .find(|row| row.name == "shell")
            .cloned()
            .expect("the shell is a row");
        assert!(
            row.totals.gpu > 0.0,
            "the sample's gpu share reaches the row"
        );
    }

    #[test]
    fn a_state_letter_reads_as_a_word() {
        assert_eq!(state_name('R'), "Running");
        assert_eq!(state_name('Z'), "Zombie");
        assert_eq!(state_name('?'), "?");
    }
}
