//! Application state and the logic behind it.
//!
//! The drawing lives in [`crate::ui`], which reads this state and turns input
//! into calls on it. Everything that can be tested without a window stays
//! here.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::apps::{self, Applications, Row, SortKey, ViewMode};
use crate::config::Config;
use crate::desktop::Index;
use crate::sample::{ProcessDetails, Sample, Signal};
use crate::sampler::Sampler;
use crate::theme::{self, Theme};

/// Frames a smoke run draws before it closes the window.
const SMOKE_FRAMES: u32 = 3;

/// The size the window opens at, in points.
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [1_040.0, 660.0];

/// The smallest the window may be made, in points.
pub const MIN_WINDOW_SIZE: [f32; 2] = [560.0, 360.0];

/// How many samples the history of the graphs holds.
///
/// It is a ring of readings rather than a fixed span of time, so a shorter
/// refresh fills the graphs faster and shows a shorter past. Two minutes at
/// the default rate is what the strip across the top covers.
pub const HISTORY: usize = 120;

/// The side of an application's icon in a row, in points.
pub const ICON_SIZE: f32 = 16.0;

/// A process the window is showing the details of.
pub struct Details {
    /// What was read about it.
    pub info: ProcessDetails,
    /// The window class of its window, if it has one.
    pub wm_class: Option<String>,
    /// When it was read, so the panel can be refreshed without being reopened.
    pub read: Instant,
}

/// The process the window is about to act on, and what it was asked to do.
pub struct Confirmation {
    /// The processes the action lands on.
    pub pids: Vec<i32>,
    /// The name shown in the question.
    pub name: String,
    /// The signal to send when the question is answered.
    pub signal: Signal,
}

impl Confirmation {
    /// The question the window shows.
    ///
    /// A force stop is the one action that cannot be taken back, so it asks
    /// first and says what it will cost.
    pub fn question(&self) -> String {
        let count = self.pids.len();
        if self.signal == Signal::Kill {
            if count == 1 {
                format!(
                    "Force stop {}? Its unsaved work is lost, and the kernel \
                     will not let it clean up.",
                    self.name
                )
            } else {
                format!(
                    "Force stop {count} processes? Their unsaved work is lost, \
                     and the kernel will not let them clean up."
                )
            }
        } else if count == 1 {
            format!("Send {} to {}?", self.signal.short_name(), self.name)
        } else {
            format!("Send {} to {count} processes?", self.signal.short_name())
        }
    }

    /// The label the button that goes ahead carries.
    pub fn action(&self) -> &'static str {
        if self.signal == Signal::Kill {
            "Force Stop"
        } else {
            "Send"
        }
    }
}

/// What the window is showing of the system.
pub struct App {
    /// Whether the window closes itself after [`SMOKE_FRAMES`] frames.
    pub(crate) smoke: bool,
    /// Whether settings are written to disk.
    ///
    /// A smoke run never writes: it must behave the same on every machine, and
    /// CI runs it on machines whose settings are not its own.
    pub(crate) persist: bool,
    /// Frames drawn so far, counted only during a smoke run.
    pub(crate) frames: u32,
    /// Settings, as last loaded or saved.
    pub(crate) config: Config,
    /// The most recent sample.
    pub(crate) sample: Sample,
    /// The reader thread, and the samples it has sent.
    pub(crate) sampler: Sampler,
    /// The desktop entries, read once.
    pub(crate) entries: Index,
    /// Which processes belong to which application.
    pub(crate) applications: Applications,
    /// The rows the table draws, rebuilt when the sample or a setting changes.
    pub(crate) rows: Vec<Row>,
    /// The processes the user folded away.
    pub(crate) folded: HashSet<i32>,
    /// The application heads the user opened, which start folded.
    ///
    /// An application's process tree is folded away until it is asked for:
    /// that is what makes the tree view readable on a desktop, where every
    /// program brings a handful of helpers.
    pub(crate) opened: HashSet<i32>,
    /// The rows' last state, which decides whether they have to be rebuilt.
    pub(crate) built: Built,
    /// The colour theme.
    pub(crate) theme: Theme,
    /// What the table is showing.
    pub(crate) view: ViewMode,
    /// The column the table is sorted by.
    pub(crate) sort: SortKey,
    /// Whether the sort runs the other way round.
    pub(crate) descending: bool,
    /// The text the filter box holds.
    pub(crate) filter: String,
    /// The processes the selection holds.
    pub(crate) selection: Vec<i32>,
    /// The row a click last selected, which a shift click extends from.
    pub(crate) anchor: Option<i32>,
    /// The details panel, when it is open.
    pub(crate) details: Option<Details>,
    /// The question the window is waiting on an answer to.
    pub(crate) confirmation: Option<Confirmation>,
    /// Bumped whenever the rows are known to be stale, which a change of
    /// state that no other field records needs.
    pub(crate) generation: u64,
    /// The CPU history, one reading per sample.
    pub(crate) cpu_history: Vec<f32>,
    /// The CPU history of each core, one reading per sample.
    pub(crate) core_history: Vec<Vec<f32>>,
    /// The memory history.
    pub(crate) memory_history: Vec<f32>,
    /// The GPU history.
    pub(crate) gpu_history: Vec<f32>,
    /// The network receive history.
    pub(crate) net_read_history: Vec<f32>,
    /// The network send history.
    pub(crate) net_write_history: Vec<f32>,
    /// The disk read history.
    pub(crate) disk_read_history: Vec<f32>,
    /// The disk write history.
    pub(crate) disk_write_history: Vec<f32>,
    /// The highest reading each graph has seen, so its scale is steady.
    pub(crate) scales: Scales,
    /// Textures of application icons, by the name the entry gives them.
    pub(crate) icons: crate::icons::Icons,
    /// Whether the window shows the process table's column of application
    /// icons.
    pub(crate) show_icons: bool,
    /// The message below the tool bar, such as an error.
    pub(crate) status: Option<String>,
}

/// The history's scales, so a graph does not jump on every sample.
#[derive(Clone, Copy, Debug)]
pub struct Scales {
    /// The largest network rate the network graph has drawn, in bytes a
    /// second.
    pub net: f32,
    /// The largest storage rate the storage graph has drawn.
    pub disk: f32,
}

impl Default for Scales {
    fn default() -> Self {
        Self {
            net: 1.0,
            disk: 1.0,
        }
    }
}

/// What the rows were built from, which decides whether they are stale.
///
/// Rebuilding the tree for every frame would cost a walk of every process on
/// every frame; rebuilding it when a sample or a setting changes costs one
/// walk per refresh. The generation is knocked on by anything that changes
/// what the rows are, which is what a fold or an unfold is: the two sets can
/// swap a row between them without either growing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Built {
    /// How many samples had arrived when the rows were built.
    pub samples: u64,
    /// A counter bumped whenever the rows are known to be stale.
    pub generation: u64,
    /// Whether the view was the tree.
    pub view: ViewMode,
    /// The column the rows were sorted by.
    pub sort: SortKey,
    /// The direction they were sorted in.
    pub descending: bool,
    /// The rows whose children were hidden.
    pub folded: u64,
    /// Whether kernel threads were shown.
    pub kernel_threads: bool,
}

impl App {
    /// Window title and human-readable app name.
    pub const TITLE: &str = "Process Control";

    /// Application id. It names the desktop entry on Linux, so the compositor
    /// can match the window to its icon.
    pub const ID: &str = "process-control";

    /// Creates the app. `smoke` makes it close itself after a few frames,
    /// which CI uses to prove a build starts and renders.
    pub fn new(egui_ctx: &egui::Context, smoke: bool) -> Self {
        let config = if smoke {
            Config::default()
        } else {
            Config::load()
        };
        let theme = config
            .theme
            .as_deref()
            .and_then(Theme::from_name)
            .unwrap_or_else(Theme::first_run);
        theme::apply(egui_ctx, theme);
        let sampler = Sampler::start(config.refresh_span());
        Self {
            smoke,
            persist: !smoke,
            frames: 0,
            sort: config
                .sort
                .as_deref()
                .and_then(SortKey::from_name)
                .unwrap_or_default(),
            descending: config.descending.unwrap_or(false),
            view: if config.group_apps.unwrap_or(true) {
                ViewMode::Applications
            } else {
                ViewMode::Tree
            },
            folded: HashSet::new(),
            opened: HashSet::new(),
            show_icons: config.show_icons.unwrap_or(true),
            theme,
            config,
            sample: Sample::default(),
            sampler,
            entries: Index::load(),
            applications: Applications::new(&Sample::default(), &[], &Index::default()),
            rows: Vec::new(),
            built: Built::default(),
            filter: String::new(),
            selection: Vec::new(),
            anchor: None,
            details: None,
            confirmation: None,
            generation: 0,
            cpu_history: Vec::new(),
            core_history: Vec::new(),
            memory_history: Vec::new(),
            gpu_history: Vec::new(),
            net_read_history: Vec::new(),
            net_write_history: Vec::new(),
            disk_read_history: Vec::new(),
            disk_write_history: Vec::new(),
            scales: Scales::default(),
            icons: crate::icons::Icons::default(),
            status: None,
        }
    }

    /// Takes the sample that arrived, if one has.
    ///
    /// Returns whether anything changed, which the caller turns into a
    /// repaint: a window that drew nothing new has no reason to draw again.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.sampler.poll();
        if let Some(sample) = self.sampler.take() {
            self.adopt(sample);
            changed = true;
        }
        changed
    }

    /// Takes a sample as the one the window shows.
    fn adopt(&mut self, sample: Sample) {
        self.applications = Applications::new(&sample, &sample.windows, &self.entries);
        self.applications.resolve(&sample);
        self.push_history(&sample);
        self.sample = sample;
        // The rows stand for numbers that just changed, so they are rebuilt
        // even though nothing about the settings did.
        self.built = Built::default();
        self.refresh_details();
    }

    /// Adds the sample to the graphs' history, dropping the oldest reading.
    fn push_history(&mut self, sample: &Sample) {
        push(&mut self.cpu_history, sample.cpu);
        push(&mut self.memory_history, sample.memory_fraction() * 100.0);
        push(&mut self.gpu_history, sample.gpu.unwrap_or(0.0));
        push(&mut self.net_read_history, sample.net_read as f32);
        push(&mut self.net_write_history, sample.net_write as f32);
        push(&mut self.disk_read_history, sample.disk_read as f32);
        push(&mut self.disk_write_history, sample.disk_write as f32);
        self.core_history.resize(sample.cores.len(), Vec::new());
        for (index, core) in sample.cores.iter().enumerate() {
            push(&mut self.core_history[index], *core);
        }
        self.scales.net = self
            .scales
            .net
            .max(sample.net_read as f32)
            .max(sample.net_write as f32)
            .max(1.0);
        self.scales.disk = self
            .scales
            .disk
            .max(sample.disk_read as f32)
            .max(sample.disk_write as f32)
            .max(1.0);
    }

    /// Fills the graphs with a made-up history, which the picture a
    /// headless test draws needs and a real run fills from its own samples.
    #[cfg(test)]
    pub fn push_histories_for_preview(&mut self) {
        for index in 0..crate::app::HISTORY {
            let wave = (index as f32 * 0.21).sin();
            let sample = Sample {
                cpu: 34.0 + wave * 18.0,
                memory_used: self.sample.memory_used,
                memory_total: self.sample.memory_total,
                gpu: Some(12.0 + wave * 9.0),
                net_read: f64::from(420_000.0 + wave * 260_000.0),
                net_write: f64::from(90_000.0 + wave * 60_000.0),
                disk_read: f64::from(1_400_000.0 + wave * 900_000.0),
                disk_write: f64::from(320_000.0 + wave * 210_000.0),
                cores: self.sample.cores.clone(),
                ..Sample::default()
            };
            self.push_history(&sample);
        }
    }

    /// Folds a section away, which the picture a test draws uses.
    #[cfg(test)]
    pub fn fold_section_for_preview(&mut self, section: crate::apps::Section) {
        self.toggle_section(section);
    }

    /// Takes a sample as the one the window shows, which a test uses.
    #[cfg(test)]
    pub fn adopt_for_test(&mut self, sample: Sample) {
        self.adopt(sample);
    }

    /// Reads the details of the process the panel shows, when it shows one.
    ///
    /// The panel follows its process rather than a copy of what it was when it
    /// was opened, so the numbers in it are as fresh as the table's. A process
    /// that exited closes the panel.
    pub fn refresh_details(&mut self) {
        let Some(details) = &self.details else {
            return;
        };
        let pid = details.info.pid;
        match crate::platform::details(pid) {
            Some(mut info) => {
                info.wm_class = self
                    .sample
                    .windows
                    .iter()
                    .find(|window| window.pid == pid)
                    .and_then(|window| window.keys().first().cloned());
                if let Some(details) = &mut self.details {
                    details.info = info;
                    details.read = Instant::now();
                }
            }
            None => {
                self.details = None;
                self.status = Some(format!("Process {pid} exited."));
            }
        }
    }

    /// Rebuilds the rows when what they were built from has changed.
    pub fn rebuild(&mut self) {
        let state = Built {
            samples: self.sampler.received(),
            view: self.view,
            sort: self.sort,
            descending: self.descending,
            generation: self.generation,
            folded: fold_key(&self.folded, &self.opened),
            kernel_threads: self.config.show_kernel_threads.unwrap_or(false),
        };
        if state == self.built && !self.rows.is_empty() {
            return;
        }
        self.built = state;
        let folded = self.folded_rows();
        self.rows = match self.view {
            ViewMode::Tree => apps::tree(
                &self.sample,
                &self.applications,
                self.config.show_kernel_threads.unwrap_or(false),
                &folded,
            ),
            ViewMode::Applications => apps::applications(&self.sample, &self.applications, &folded),
        };
        apps::sort(&mut self.rows, self.sort, self.descending);
    }

    /// The rows the filter lets through, with a row kept when anything it
    /// stands for matches so a parent whose child matches stays reachable.
    pub fn visible_rows(&self) -> Vec<usize> {
        let filter = self.filter.trim().to_lowercase();
        if filter.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let index = self.sample.index();
        (0..self.rows.len())
            .filter(|position| {
                let row = &self.rows[*position];
                row.name.to_lowercase().contains(&filter)
                    || row.pids.iter().any(|pid| {
                        pid.to_string() == filter
                            || index.get(pid).is_some_and(|index| {
                                let process = &self.sample.processes[*index];
                                process.name.to_lowercase().contains(&filter)
                                    || process
                                        .exe
                                        .as_deref()
                                        .is_some_and(|exe| exe.to_lowercase().contains(&filter))
                            })
                    })
            })
            .collect()
    }

    /// Changes what the table is sorted by, keeping the direction when the
    /// key stays and picking the one the key reads best in when it changes.
    pub fn sort_by(&mut self, key: SortKey) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = apps::default_direction(key);
        }
        self.save();
    }

    /// Folds a row away, or unfolds it.
    ///
    /// A row an application's window belongs to starts folded, so folding it
    /// back means taking it out of the opened set rather than adding it to the
    /// folded one; a row that starts open is folded the other way round.
    pub fn toggle_fold(&mut self, pid: i32) {
        if self.is_folded(pid) {
            self.folded.remove(&pid);
            self.opened.insert(pid);
        } else {
            self.folded.insert(pid);
            self.opened.remove(&pid);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// Folds a section away, or unfolds it.
    ///
    /// A heading folds by its own key rather than by a pid, which is why the
    /// two section keys are negative: no process id can take them.
    pub fn toggle_section(&mut self, section: crate::apps::Section) {
        let key = match section {
            crate::apps::Section::Applications => crate::apps::SECTION_APPLICATIONS,
            crate::apps::Section::Processes => crate::apps::SECTION_PROCESSES,
        };
        if !self.folded.remove(&key) {
            self.folded.insert(key);
        }
        // The section keys are not process ids, so opening one must not look
        // like opening a window owner.
        self.opened.remove(&key);
    }

    /// Whether a row's children are hidden.
    ///
    /// An application head is folded unless it was opened; everything else is
    /// open unless it was folded.
    pub fn is_folded(&self, pid: i32) -> bool {
        if self.folded.contains(&pid) {
            return true;
        }
        self.applications.owns_window(pid) && !self.opened.contains(&pid)
    }

    /// The set of rows whose children are hidden, which the row builders take.
    pub fn folded_rows(&self) -> HashSet<i32> {
        let mut folded = self.folded.clone();
        for pid in self.sample.processes.iter().map(|process| process.pid) {
            if self.is_folded(pid) {
                folded.insert(pid);
            }
        }
        folded
    }

    /// Selects one process, dropping the rest.
    #[cfg(test)]
    pub fn select_only(&mut self, pid: i32) {
        self.selection = vec![pid];
        self.anchor = Some(pid);
    }

    /// Selects a whole row: an application's own processes and everything
    /// beneath them.
    ///
    /// A row stands for more than one process when it is an application, and
    /// acting on it should act on all of them, which is what a stop from the
    /// application view must do.
    pub fn select_row(&mut self, position: usize) {
        let Some(row) = self.rows.get(position) else {
            return;
        };
        // A heading stands for no process, so there is nothing to select.
        if row.is_heading() {
            return;
        }
        let mut pids = row.pids.clone();
        let own = row.pids.first().copied().unwrap_or_default();
        pids.extend(self.descendants(own));
        pids.sort_unstable();
        pids.dedup();
        self.selection = pids;
        self.anchor = Some(own);
    }

    /// Adds a row to the selection, or takes it out.
    pub fn toggle_row(&mut self, position: usize) {
        let Some(row) = self.rows.get(position) else {
            return;
        };
        if row.is_heading() {
            return;
        }
        let own = row.pids.first().copied().unwrap_or_default();
        if self.selection.contains(&own) {
            self.selection.retain(|pid| *pid != own);
        } else {
            self.selection.extend(row.pids.iter().copied());
            self.anchor = Some(own);
        }
    }

    /// Extends the selection from the anchor to the row at `position`.
    pub fn extend_to(&mut self, position: usize) {
        if position >= self.rows.len() {
            return;
        }
        let Some(anchor) = self.anchor else {
            return self.select_row(position);
        };
        let Some(from) = self
            .rows
            .iter()
            .position(|row| row.pids.first() == Some(&anchor))
        else {
            return self.select_row(position);
        };
        let (start, end) = if from <= position {
            (from, position)
        } else {
            (position, from)
        };
        let mut pids = Vec::new();
        for row in &self.rows[start..=end] {
            // A heading inside the range stands for no process, so it adds
            // nothing rather than clearing what the range covers.
            pids.extend(row.pids.iter().copied());
        }
        pids.sort_unstable();
        pids.dedup();
        self.selection = pids;
    }

    /// Every process beneath `pid` in the sample, however deeply.
    pub fn descendants(&self, pid: i32) -> Vec<i32> {
        let mut found = Vec::new();
        let mut frontier = vec![pid];
        while let Some(current) = frontier.pop() {
            for process in &self.sample.processes {
                if process.ppid == current
                    && process.pid != current
                    && !found.contains(&process.pid)
                {
                    found.push(process.pid);
                    frontier.push(process.pid);
                }
            }
        }
        found
    }

    /// Asks to send a signal to the selection.
    ///
    /// A force stop asks first, since it cannot be undone; the other signals
    /// go straight out, and the status line says what happened.
    pub fn send_signal(&mut self, signal: Signal) {
        if self.selection.is_empty() {
            return;
        }
        let name = self
            .rows
            .iter()
            .find(|row| {
                row.pids
                    .first()
                    .is_some_and(|pid| self.selection.contains(pid))
            })
            .map(|row| row.name.clone())
            .unwrap_or_else(|| self.selection[0].to_string());
        if signal == Signal::Kill {
            self.confirmation = Some(Confirmation {
                pids: self.selection.clone(),
                name,
                signal,
            });
            return;
        }
        self.deliver(signal);
    }

    /// Sends a signal to every selected process and says how it went.
    pub fn deliver(&mut self, signal: Signal) {
        let pids = match &self.confirmation {
            Some(confirmation) if confirmation.signal == signal => confirmation.pids.clone(),
            _ => self.selection.clone(),
        };
        self.confirmation = None;
        let mut failed = Vec::new();
        for pid in &pids {
            // The app's own pid is left alone: a stop of the task manager is
            // not what a click on its own row should do.
            if *pid == std::process::id() as i32 {
                continue;
            }
            if let Err(error) = crate::platform::signal(*pid, signal) {
                failed.push(error);
            }
        }
        self.status = Some(if failed.is_empty() {
            format!(
                "Sent {} to {} process{}",
                signal.short_name(),
                pids.len(),
                if pids.len() == 1 { "" } else { "es" }
            )
        } else {
            failed.join("; ")
        });
        if signal.stops() || signal == Signal::Stop {
            self.selection.clear();
        }
        self.sampler.refresh();
    }

    /// Opens the details of a row's own process, or of the one it stands for.
    pub fn open_details(&mut self, position: usize) {
        let Some(row) = self.rows.get(position) else {
            return;
        };
        // An application row stands for several processes; the panel opens
        // the busiest of them, which is the one the user is watching.
        let pid = self.sample.processes.get(row.main).map_or_else(
            || row.pids.first().copied().unwrap_or_default(),
            |process| process.pid,
        );
        let Some(info) = crate::platform::details(pid) else {
            self.status = Some(format!("Process {pid} exited."));
            return;
        };
        let wm_class = self
            .sample
            .windows
            .iter()
            .find(|window| window.pid == pid)
            .and_then(|window| window.keys().first().cloned());
        self.details = Some(Details {
            info,
            wm_class,
            read: Instant::now(),
        });
    }

    /// Changes how often the list is read, keeping the setting.
    pub fn set_refresh(&mut self, milliseconds: u32) {
        self.config.refresh = Some(milliseconds);
        self.sampler
            .set_interval(Duration::from_millis(u64::from(milliseconds)));
        self.save();
    }

    /// Changes the theme, which is applied at once.
    pub fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.theme = theme;
        self.config.theme = Some(theme.name().to_string());
        theme::apply(ctx, theme);
        self.save();
    }

    /// Writes the settings, unless this is a smoke run.
    pub fn save(&mut self) {
        if !self.persist {
            return;
        }
        self.config.sort = Some(self.sort.name().to_string());
        self.config.descending = Some(self.descending);
        self.config.group_apps = Some(self.view == ViewMode::Applications);
        self.config.show_icons = Some(self.show_icons);
        if let Err(error) = self.config.save() {
            self.status = Some(error);
        }
    }

    /// How many processes the machine has, of which how many are running.
    pub fn process_summary(&self) -> String {
        format!(
            "{} processes, {} running",
            self.sample.process_count, self.sample.running
        )
    }
}

/// A number that stands for the contents of the two fold sets together.
///
/// The two can swap a row between them without either growing, so their
/// lengths alone would miss the change.
fn fold_key(folded: &HashSet<i32>, opened: &HashSet<i32>) -> u64 {
    let mut key = folded.len() as u64 * 0x100000001;
    for pid in folded {
        key = key.wrapping_mul(31).wrapping_add(*pid as u64);
    }
    key = key.wrapping_mul(131);
    for pid in opened {
        key = key.wrapping_mul(31).wrapping_add(*pid as u64);
    }
    key
}

/// Adds one reading to a history, dropping the oldest.
fn push(history: &mut Vec<f32>, value: f32) {
    if history.len() >= HISTORY {
        history.remove(0);
    }
    history.push(value);
}

impl eframe::App for App {
    /// The colour the renderer clears to, which shows wherever no panel
    /// covers it.
    ///
    /// eframe's own default is a dark grey, so a light theme drew its window
    /// over a dark border; this follows the chosen palette.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let palette = self.theme.palette();
        egui::Color32::from_rgb(
            palette.background.0,
            palette.background.1,
            palette.background.2,
        )
        .to_normalized_gamma_f32()
    }

    /// Called before every frame, and on its own while the window is hidden:
    /// the sample that arrived is taken here, so a window that is brought
    /// back shows the machine as it is now rather than as it was.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.poll() {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.rebuild();
        crate::ui::draw(self, ui);
        if self.smoke {
            self.frames += 1;
            if self.frames >= SMOKE_FRAMES {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::ProcessInfo;

    fn process(pid: i32, ppid: i32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            name: name.to_string(),
            virtual_memory: 4096,
            ..ProcessInfo::default()
        }
    }

    /// An app with no window and nothing read yet, for the logic to be tested
    /// on its own.
    fn app() -> App {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.sample = Sample {
            processes: vec![
                process(1, 0, "init"),
                process(2, 1, "shell"),
                process(3, 2, "child"),
                process(4, 1, "other"),
            ],
            ..Sample::default()
        };
        app.applications = Applications::new(&app.sample, &[], &Index::default());
        app.applications.resolve(&app.sample);
        app.rebuild();
        app
    }

    #[test]
    fn a_row_selects_its_whole_subtree() {
        let mut app = app();
        let row = app.rows.iter().position(|row| row.pids == [2]).unwrap();
        app.select_row(row);
        assert_eq!(app.selection, [2, 3], "the child comes with its parent");
    }

    #[test]
    fn the_selection_extends_over_a_range_of_rows() {
        let mut app = app();
        let first = app
            .rows
            .iter()
            .position(|row| !row.is_heading())
            .expect("there is a process row");
        let last = app.rows.len() - 1;
        // Every process the range covers, which is what the selection should
        // hold: the headings in it stand for none.
        let expected: usize = {
            let mut pids: Vec<i32> = app.rows[first..=last]
                .iter()
                .flat_map(|row| row.pids.iter().copied())
                .collect();
            pids.sort_unstable();
            pids.dedup();
            pids.len()
        };
        app.select_row(first);
        app.extend_to(last);
        assert_eq!(app.selection.len(), expected);
        // Extending backwards from the last row covers the same range.
        app.select_row(last);
        app.extend_to(first);
        assert_eq!(app.selection.len(), expected);
    }

    #[test]
    fn a_heading_cannot_be_selected() {
        let mut app = app();
        let heading = app
            .rows
            .iter()
            .position(crate::apps::Row::is_heading)
            .expect("the applications view has a heading");
        app.select_only(1);
        app.select_row(heading);
        assert_eq!(app.selection, [1], "the selection is unchanged");
        app.toggle_row(heading);
        assert_eq!(app.selection, [1]);
    }

    #[test]
    fn the_filter_keeps_the_rows_that_match() {
        let mut app = app();
        app.filter = "child".to_string();
        let visible = app.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(app.rows[visible[0]].name, "child");
        app.filter = "2".to_string();
        assert!(!app.visible_rows().is_empty(), "a pid is a filter too");
        app.filter = String::new();
        assert_eq!(app.visible_rows().len(), app.rows.len());
    }

    #[test]
    fn folding_a_row_hides_its_children() {
        let mut app = app();
        // The tree view is the one that nests, so folding is tested there.
        app.view = ViewMode::Tree;
        app.rebuild();
        let before = app.rows.len();
        app.toggle_fold(2);
        app.rebuild();
        assert!(app.rows.len() < before);
        assert!(app.is_folded(2));
        app.toggle_fold(2);
        app.rebuild();
        assert_eq!(app.rows.len(), before);
    }

    #[test]
    fn an_applications_tree_starts_folded() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.sample = Sample {
            processes: vec![
                process(1, 0, "init"),
                process(100, 1, "firefox"),
                process(101, 100, "firefox helper"),
            ],
            windows: vec![crate::sample::Window {
                id: 1,
                pid: 100,
                class: "Navigator".to_string(),
                instance: "firefox".to_string(),
                title: "A page".to_string(),
            }],
            ..Sample::default()
        };
        app.applications = Applications::new(&app.sample, &app.sample.windows, &Index::default());
        app.applications.resolve(&app.sample);
        app.view = ViewMode::Tree;
        app.generation += 1;
        app.rebuild();
        assert!(app.is_folded(100), "the application starts folded");
        assert!(
            !app.rows.iter().any(|row| row.name == "firefox helper"),
            "its helper is hidden until it is opened: {:?}",
            app.rows.iter().map(|row| &row.name).collect::<Vec<_>>()
        );
        app.toggle_fold(100);
        app.rebuild();
        assert!(!app.is_folded(100));
        assert!(app.rows.iter().any(|row| row.name == "firefox helper"));
    }

    #[test]
    fn the_application_view_also_starts_folded() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.sample = Sample {
            processes: vec![
                process(1, 0, "init"),
                process(100, 1, "firefox"),
                process(101, 100, "Web Content"),
            ],
            windows: vec![crate::sample::Window {
                id: 1,
                pid: 100,
                class: "Navigator".to_string(),
                instance: "firefox".to_string(),
                title: "A page".to_string(),
            }],
            ..Sample::default()
        };
        app.applications = Applications::new(&app.sample, &app.sample.windows, &Index::default());
        app.applications.resolve(&app.sample);
        app.view = ViewMode::Applications;
        app.rebuild();
        assert!(app.is_folded(100), "the application starts folded");
        let navigator = app
            .rows
            .iter()
            .position(|row| row.name == "Navigator")
            .expect("the application is a row");
        assert!(app.rows[navigator].folded);
        assert!(
            !app.rows.iter().any(|row| row.name == "Web Content"),
            "its helper is not a row until the application is opened"
        );
        // Selecting the row still covers the helper, which is what makes a
        // stop from this view stop the whole program.
        app.select_row(navigator);
        assert!(app.selection.contains(&101));
    }

    #[test]
    fn a_force_stop_asks_before_it_acts() {
        let mut app = app();
        app.select_only(2);
        app.send_signal(Signal::Kill);
        let question = app.confirmation.as_ref().expect("the window asks first");
        assert!(question.question().contains("Force stop"));
        assert!(question.action() == "Force Stop");
        // The process was not signalled yet.
        assert!(app.confirmation.is_some());
        app.confirmation = None;
    }

    #[test]
    fn a_terminate_needs_no_confirmation() {
        let mut app = app();
        app.select_only(i32::MAX - 1);
        app.send_signal(Signal::Term);
        assert!(app.confirmation.is_none());
        let status = app
            .status
            .clone()
            .expect("the status line says what happened");
        assert!(!status.is_empty());
    }

    #[test]
    fn the_app_never_stops_itself() {
        let mut app = app();
        let own = std::process::id() as i32;
        app.selection = vec![own];
        app.deliver(Signal::Kill);
        // Still running, which is the point.
        assert!(app.status.is_some());
    }

    #[test]
    fn the_history_holds_the_newest_reading_and_drops_the_oldest() {
        let mut history = Vec::new();
        for value in 0..(HISTORY + 10) {
            push(&mut history, value as f32);
        }
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history.last(), Some(&((HISTORY + 9) as f32)));
        assert_eq!(history.first(), Some(&10.0));
    }

    #[test]
    fn a_sort_click_flips_the_direction_of_the_same_key() {
        let mut app = app();
        app.sort = SortKey::Cpu;
        app.descending = true;
        app.sort_by(SortKey::Cpu);
        assert!(!app.descending);
        app.sort_by(SortKey::Name);
        assert_eq!(app.sort, SortKey::Name);
        assert!(!app.descending, "a name reads best from a to z");
    }
}
