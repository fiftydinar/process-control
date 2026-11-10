//! The desktop entries, indexed by the names a window is matched against.
//!
//! A process does not know what application it is. The desktop's own files do:
//! a `.desktop` entry names a program, its icon, and the window class it
//! starts as (`StartupWMClass`). Reading them is what turns a row named
//! `firefox` into a row named Firefox with the Firefox icon, and it works for
//! every program that ships a desktop entry, whatever package format carried
//! it there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a desktop entry says about its application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// The name shown in menus, in the user's language when it has one.
    pub name: String,
    /// The generic name, such as "Web Browser".
    pub generic_name: String,
    /// The icon's name or path.
    pub icon: String,
    /// The window class the program starts as.
    pub startup_wm_class: String,
    /// The command the entry runs, without its arguments.
    pub exec: String,
    /// Whether the entry should be hidden from menus.
    pub hidden: bool,
    /// The file it was read from.
    pub path: PathBuf,
}

impl Entry {
    /// Whether the entry names a program to launch, rather than a service or
    /// a link.
    pub fn is_application(&self) -> bool {
        !self.hidden && !self.name.is_empty()
    }
}

/// The desktop entries, with the lookups the app makes on every sample.
#[derive(Debug, Default)]
pub struct Index {
    /// Every application entry, in the order they were found.
    entries: Vec<Entry>,
    /// The entry each window class, lower cased, names.
    by_class: HashMap<String, usize>,
    /// The entry each executable name, lower cased, names.
    by_exec: HashMap<String, usize>,
}

impl Index {
    /// Reads every desktop entry the system and the user have.
    ///
    /// This happens off the drawing thread, once per launch: an entry does
    /// not change while the app runs, and reading a few hundred files is the
    /// one cost in the sampler that is not paid per sample.
    pub fn load() -> Self {
        Self::load_from(&directories())
    }

    /// Reads the entries in the given directories, in order.
    fn load_from(directories: &[PathBuf]) -> Self {
        let mut index = Self::default();
        for directory in directories {
            index.read_dir(directory);
        }
        index
    }

    /// Reads the entries in one directory, descending into it.
    ///
    /// The desktop entry specification allows an entry in a subdirectory of
    /// `applications`, such as `kde4/foo.desktop`, and gives it the desktop
    /// file ID `kde4-foo.desktop`. Dropping those loses applications that
    /// install themselves below the top level.
    fn read_dir(&mut self, directory: &Path) {
        let mut paths = Vec::new();
        collect_desktop_files(directory, 0, &mut paths);
        // The order the file system lists them in is not stable, and an entry
        // found twice should be the one the desktop itself would use.
        paths.sort();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(entry) = parse(&text, &path) else {
                continue;
            };
            if !entry.is_application() {
                continue;
            }
            self.insert(entry);
        }
    }

    /// Adds one entry and the names it can be found by.
    fn insert(&mut self, entry: Entry) {
        let position = self.entries.len();
        for key in names_of(&entry) {
            // The first entry that claims a name keeps it: the user's own
            // directory is read before the system's, and a program installed
            // twice should be named by the copy that was found first.
            self.by_class.entry(key).or_insert(position);
        }
        if let Some(name) = exec_name(&entry.exec) {
            let key = name.to_lowercase();
            // Several entries can run one program: Steam's own entry and a
            // shortcut for each game all run `steam`. The shortcut is named
            // and iconed after the game, and sorts before the program's own
            // entry, so it must not take the executable's name from it.
            match self.by_exec.get(&key).copied() {
                Some(existing) => {
                    if !names_program(&self.entries[existing], &name)
                        && names_program(&entry, &name)
                    {
                        self.by_exec.insert(key, position);
                    }
                }
                None => {
                    self.by_exec.insert(key, position);
                }
            }
        }
        self.entries.push(entry);
    }

    /// The entry a process belongs to, by the window class it publishes or
    /// the executable it runs.
    pub fn matching(&self, classes: &[String], executable: Option<&str>) -> Option<&Entry> {
        for class in classes {
            if let Some(entry) = self.by_class_name(class) {
                return Some(entry);
            }
        }
        let executable = executable?;
        self.by_exec_name(executable)
    }

    /// The entry a window class, or a desktop entry id, names.
    ///
    /// The name is tried as it is and then without a `.desktop` suffix: a
    /// Wayland application names itself after its entry id,
    /// `al.opencode.desktop`, which would otherwise be looked for as a class
    /// and never found, leaving the row with a placeholder icon.
    pub fn by_class_name(&self, class: &str) -> Option<&Entry> {
        let class = class.trim();
        if class.is_empty() {
            return None;
        }
        if let Some(index) = self.by_class.get(&class.to_lowercase()) {
            return self.entries.get(*index);
        }
        let bare = class
            .strip_suffix(".desktop")
            .or_else(|| class.strip_suffix(".Desktop"))?;
        self.by_class
            .get(&bare.to_lowercase())
            .and_then(|index| self.entries.get(*index))
    }

    /// The entry a program's executable name names.
    pub fn by_exec_name(&self, executable: &str) -> Option<&Entry> {
        self.by_exec
            .get(&executable.to_lowercase())
            .and_then(|index| self.entries.get(*index))
    }

    /// Every entry, in the order they were read.
    #[cfg(test)]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many applications were found, which the About line reports.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Whether `entry` names the program an executable runs, rather than a
/// shortcut that merely launches it.
///
/// A shortcut and the program it launches can share one executable: Steam
/// and a desktop shortcut for a game both run `steam`, while the shortcut's
/// name, icon and window class are the game's. The program's own entry is the
/// one that calls itself after its executable, through its name, its
/// `StartupWMClass` or its file name, so it is the one the executable name
/// resolves to.
fn names_program(entry: &Entry, executable: &str) -> bool {
    let executable = executable.trim();
    if executable.is_empty() {
        return false;
    }
    let stem = entry.path.file_stem().and_then(|stem| stem.to_str());
    entry
        .startup_wm_class
        .trim()
        .eq_ignore_ascii_case(executable)
        || entry.name.trim().eq_ignore_ascii_case(executable)
        || stem.is_some_and(|stem| stem.eq_ignore_ascii_case(executable))
}

/// The names a window may carry to be matched to `entry`, lower cased.
///
/// The entry's own file name is its desktop id, `org.mozilla.firefox.desktop`,
/// and some programs set their window class to that id rather than to the
/// `StartupWMClass` the entry names. Both are offered, so a program that
/// names itself either way is found.
fn names_of(entry: &Entry) -> Vec<String> {
    let mut names = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    };
    add(&entry.startup_wm_class);
    if let Some(stem) = entry.path.file_stem().and_then(|stem| stem.to_str()) {
        add(stem);
    }
    if let Some(id) = desktop_id(&entry.path) {
        add(&id);
        add(id.strip_suffix(".desktop").unwrap_or(&id));
    }
    names
}

/// The desktop file ID of a path under an `applications` directory.
///
/// The ID is the path relative to `applications`, with `/` turned into `-`:
/// `kde4/foo.desktop` is `kde4-foo.desktop`. An entry outside an
/// `applications` directory has no ID and is matched by its file name alone.
fn desktop_id(path: &Path) -> Option<String> {
    let components: Vec<_> = path.components().collect();
    let start = components
        .iter()
        .rposition(|component| component.as_os_str() == "applications")?;
    let relative: Vec<&str> = components[start + 1..]
        .iter()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    if relative.is_empty() {
        return None;
    }
    Some(relative.join("-"))
}

/// Collects every `.desktop` file under `directory`, to a bounded depth.
fn collect_desktop_files(directory: &Path, depth: usize, paths: &mut Vec<PathBuf>) {
    const MAX_DEPTH: usize = 16;
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
        {
            paths.push(path);
        } else if depth < MAX_DEPTH && entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            collect_desktop_files(&path, depth + 1, paths);
        }
    }
}

/// The program a desktop entry's `Exec` line runs, as a file name.
///
/// The line can carry field codes such as `%f` and `%U`, which stand for files
/// and URLs and are not part of the program's name.
pub fn exec_name(exec: &str) -> Option<String> {
    let mut parts = exec.split_whitespace();
    let program = parts.next()?;
    // A command may be wrapped in `env VAR=value`, which the program follows.
    if program == "env" {
        return parts
            .find(|part| !part.contains('=') && !part.starts_with('-'))
            .and_then(|part| part.rsplit('/').next())
            .map(str::to_string);
    }
    program
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && !name.starts_with('%'))
        .map(str::to_string)
}

/// Reads one desktop entry file.
///
/// Only the `[Desktop Entry]` group is read: a file also carries action
/// groups, which name the same keys for something else.
pub fn parse(text: &str, path: &Path) -> Option<Entry> {
    let mut group = String::new();
    let mut entry = Entry {
        path: path.to_path_buf(),
        ..Entry::default()
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            group = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            continue;
        }
        if group != "Desktop Entry" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            // The translated names are keys of their own, `Name[xx]`, and
            // the untranslated one is what this reads: a wrong language is
            // worse than the name its authors wrote first.
            "Name" => entry.name = value.to_string(),
            "GenericName" => entry.generic_name = value.to_string(),
            "Icon" => entry.icon = value.to_string(),
            "StartupWMClass" => entry.startup_wm_class = value.to_string(),
            "Exec" => entry.exec = value.to_string(),
            // `NoDisplay` hides an entry from menus, but the application
            // still exists: its name, icon and window class are what a running
            // process is matched with.
            "Hidden" => entry.hidden = value.eq_ignore_ascii_case("true"),
            // A Link or Directory entry names a URL or a folder, not a
            // program to run.
            "Type" if !value.eq_ignore_ascii_case("Application") => entry.hidden = true,
            _ => {}
        }
    }
    (!entry.name.is_empty()).then_some(entry)
}

/// The directories desktop entries are read from.
///
/// The user's own directory comes first, so a program the user installed
/// themselves is named by their copy rather than by the system's. It is read
/// from `XDG_DATA_HOME` and `XDG_DATA_DIRS` rather than a fixed path, which is
/// what makes this work inside an Anylinux AppImage and in a sandbox that
/// mounts the host's data directories somewhere else.
fn directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(data) = dirs::data_dir() {
        push_unique(&mut directories, data.join("applications"));
    }
    // Flatpak exports the entries of its user and system installs here, and
    // those directories are not part of the host's `XDG_DATA_DIRS` unless
    // flatpak itself set it up for a sandboxed program.
    for directory in flatpak_directories() {
        push_unique(&mut directories, directory);
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
        push_unique(&mut directories, directory.join("applications"));
    }
    directories
}

/// The directories Flatpak exports its desktop entries to.
///
/// The user's install comes first, so it names an application that a system
/// install also carries. `FLATPAK_USER_DIR` and `FLATPAK_SYSTEM_DIR` move the
/// two installations from their defaults, as a distribution may.
fn flatpak_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let user = std::env::var_os("FLATPAK_USER_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|data| data.join("flatpak")));
    if let Some(user) = user {
        directories.push(user.join("exports/share/applications"));
    }
    let system = std::env::var_os("FLATPAK_SYSTEM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/flatpak"));
    directories.push(system.join("exports/share/applications"));
    directories
}

/// Adds `directory` unless it is already in the list.
fn push_unique(directories: &mut Vec<PathBuf>, directory: PathBuf) {
    if !directories.contains(&directory) {
        directories.push(directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_entry_is_read() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Firefox
GenericName=Web Browser
Icon=firefox
Exec=/usr/lib/firefox/firefox %u
StartupWMClass=Firefox
";
        let entry = parse(text, Path::new("/usr/share/applications/firefox.desktop"))
            .expect("the entry parses");
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.generic_name, "Web Browser");
        assert_eq!(entry.icon, "firefox");
        assert_eq!(entry.startup_wm_class, "Firefox");
        assert_eq!(exec_name(&entry.exec).as_deref(), Some("firefox"));
        assert!(entry.is_application());
    }

    #[test]
    fn a_desktop_id_turns_subdirectory_slashes_into_dashes() {
        assert_eq!(
            desktop_id(Path::new("/usr/share/applications/kde4/foo.desktop")),
            Some("kde4-foo.desktop".to_string())
        );
        assert_eq!(
            desktop_id(Path::new("/usr/share/applications/foo.desktop")),
            Some("foo.desktop".to_string())
        );
        assert_eq!(desktop_id(Path::new("/tmp/foo.desktop")), None);
    }

    #[test]
    fn an_entry_in_a_subdirectory_is_still_an_application() {
        let directory =
            std::env::temp_dir().join(format!("process-control-desktop-{}", std::process::id()));
        let nested = directory.join("applications").join("kde4");
        std::fs::create_dir_all(&nested).expect("the directory is made");
        std::fs::write(
            nested.join("foo.desktop"),
            "[Desktop Entry]\nType=Application\nName=Foo\n",
        )
        .expect("the entry is written");
        let mut index = Index::default();
        index.read_dir(&directory.join("applications"));
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert_eq!(index.len(), 1);
        assert!(index.by_class_name("kde4-foo").is_some());
        assert!(index.by_class_name("foo").is_some());
    }

    #[test]
    fn the_flatpak_export_directories_are_read() {
        // Flatpak's exports are not in the host's XDG_DATA_DIRS, so the
        // index has to add them itself.
        let directories = directories();
        let user = std::env::var_os("FLATPAK_USER_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::data_dir().map(|data| data.join("flatpak")))
            .map(|directory| directory.join("exports/share/applications"));
        if let Some(user) = user {
            assert!(directories.contains(&user), "{directories:?}");
        }
        let system = std::env::var_os("FLATPAK_SYSTEM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/flatpak"));
        assert!(
            directories.contains(&system.join("exports/share/applications")),
            "{directories:?}"
        );
    }

    #[test]
    fn a_flatpak_export_is_read() {
        // Flatpak installs a symlink in exports/share/applications whose name,
        // `org.example.App.desktop`, is also the window class the app sets.
        let root =
            std::env::temp_dir().join(format!("process-control-flatpak-{}", std::process::id()));
        let exports = root.join("exports/share/applications");
        let installed = root.join("app/org.example.App/current/active/export/share/applications");
        std::fs::create_dir_all(&installed).expect("the installed directory is made");
        std::fs::create_dir_all(&exports).expect("the export directory is made");
        let file = installed.join("org.example.App.desktop");
        std::fs::write(
            &file,
            "[Desktop Entry]\nType=Application\nName=Example App\nIcon=org.example.App\nExec=/usr/bin/flatpak run org.example.App\n",
        )
        .expect("the entry is written");
        std::os::unix::fs::symlink(&file, exports.join("org.example.App.desktop"))
            .expect("the export symlink is made");

        let index = Index::load_from(&[exports]);
        std::fs::remove_dir_all(&root).expect("the temporary directory is removed");

        let entry = index
            .by_class_name("org.example.App")
            .expect("the desktop file name is the window class");
        assert_eq!(entry.name, "Example App");
        assert_eq!(entry.icon, "org.example.App");
    }

    #[test]
    fn only_the_desktop_entry_group_is_read() {
        // A file also carries action groups, which repeat the same keys.
        let text = "\
[Desktop Entry]
Name=Files
Exec=nautilus --new-window %U

[Desktop Action new-window]
Name=New Window
Exec=nautilus
";
        let entry = parse(text, Path::new("/usr/share/applications/files.desktop")).unwrap();
        assert_eq!(entry.name, "Files");
        assert_eq!(entry.exec, "nautilus --new-window %U");
    }

    #[test]
    fn a_hidden_entry_is_not_an_application() {
        let text = "[Desktop Entry]\nType=Application\nName=Hidden Thing\nHidden=true\n";
        let entry = parse(text, Path::new("/tmp/hidden.desktop")).unwrap();
        assert!(!entry.is_application());
    }

    #[test]
    fn an_entry_hidden_from_menus_still_matches_a_window() {
        let text = "[Desktop Entry]\nType=Application\nName=Hidden Thing\nNoDisplay=true\nStartupWMClass=Thing\n";
        let entry = parse(text, Path::new("/tmp/no-display.desktop")).unwrap();
        assert!(entry.is_application());
        assert_eq!(entry.startup_wm_class, "Thing");
    }

    #[test]
    fn an_entry_of_another_type_is_not_an_application() {
        let text = "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://example.com\n";
        let entry = parse(text, Path::new("/tmp/link.desktop")).unwrap();
        assert!(!entry.is_application());
    }

    #[test]
    fn a_translated_name_does_not_override_the_plain_one() {
        let text = "[Desktop Entry]\nName=Terminal\nName[de]=Terminal\nExec=term\n";
        let entry = parse(text, Path::new("/tmp/term.desktop")).unwrap();
        assert_eq!(entry.name, "Terminal");
    }

    #[test]
    fn a_field_code_is_not_a_program_name() {
        assert_eq!(exec_name("%f"), None);
        assert_eq!(exec_name(""), None);
        assert_eq!(exec_name("/usr/bin/thing %f"), Some("thing".to_string()));
    }

    #[test]
    fn an_env_wrapper_names_the_program_behind_it() {
        assert_eq!(
            exec_name("env BAMF_DESKTOP_FILE_HINT=/x org.gnome.Nautilus %U").as_deref(),
            Some("org.gnome.Nautilus")
        );
    }

    /// Builds an index out of entries given as text.
    fn index_of(entries: &[(&str, &str)]) -> Index {
        let mut index = Index::default();
        for (name, text) in entries {
            let path = PathBuf::from(format!("/apps/{name}.desktop"));
            if let Some(entry) = parse(text, &path) {
                index.insert(entry);
            }
        }
        index
    }

    #[test]
    fn a_window_is_matched_by_its_class_or_its_executable() {
        let index = index_of(&[
            (
                "firefox",
                "[Desktop Entry]\nName=Firefox\nIcon=firefox\nExec=firefox %u\nStartupWMClass=Navigator\n",
            ),
            (
                "kitty",
                "[Desktop Entry]\nName=Kitty\nIcon=kitty\nExec=kitty\n",
            ),
        ]);

        let by_class = index.matching(&["Navigator".to_string()], None);
        assert_eq!(by_class.map(|entry| entry.name.as_str()), Some("Firefox"));
        // The class a window carries is often spelled differently from the
        // one the entry names, so the comparison ignores case.
        let by_case = index.matching(&["navigator".to_string()], None);
        assert_eq!(by_case.map(|entry| entry.name.as_str()), Some("Firefox"));
        let by_exec = index.matching(&[], Some("kitty"));
        assert_eq!(by_exec.map(|entry| entry.name.as_str()), Some("Kitty"));
        assert!(index.matching(&[], Some("no-such-program")).is_none());
    }

    #[test]
    fn a_shared_executable_names_the_program_that_runs_it() {
        // Steam ships one entry per game, and each runs `steam` with a game
        // id. They are named and iconed after the game and sort before
        // Steam's own entry, but the executable `steam` still has to name
        // Steam, not the first game alphabetically.
        let index = index_of(&[
            (
                "Counter-Strike 2",
                "[Desktop Entry]\nName=Counter-Strike 2\nIcon=cs2\nExec=steam steam://rungameid/730\nStartupWMClass=cs2\n",
            ),
            (
                "steam-AM",
                "[Desktop Entry]\nName=Steam\nIcon=steam\nExec=/home/user/.local/bin/steam %U\nStartupWMClass=steam\n",
            ),
        ]);

        let by_exec = index.matching(&[], Some("steam"));
        assert_eq!(by_exec.map(|entry| entry.name.as_str()), Some("Steam"));
        // The game's own window class still names the game.
        let by_class = index.matching(&["cs2".to_string()], Some("steam"));
        assert_eq!(
            by_class.map(|entry| entry.name.as_str()),
            Some("Counter-Strike 2")
        );
    }

    #[test]
    fn the_desktop_id_is_a_name_too() {
        // Some programs set their window class to the entry's own id.
        let index = index_of(&[(
            "org.example.Thing",
            "[Desktop Entry]\nName=Example Thing\nExec=thing\n",
        )]);
        let entry = index.matching(&["org.example.thing".to_string()], None);
        assert_eq!(
            entry.map(|entry| entry.name.as_str()),
            Some("Example Thing")
        );
    }

    #[test]
    fn loading_the_desktop_entries_is_quick() {
        let started = std::time::Instant::now();
        let index = Index::load();
        let took = started.elapsed();
        println!("{} entries in {took:?}", index.len());
        assert!(
            took < std::time::Duration::from_millis(250),
            "reading the entries takes {took:?}"
        );
    }

    #[test]
    fn the_system_has_desktop_entries_to_read() {
        // Whatever the test machine has installed, the entries parse.
        let index = Index::load();
        for entry in index.entries() {
            assert!(!entry.name.is_empty());
            assert!(entry.path.extension().is_some_and(|e| e == "desktop"));
        }
        assert_eq!(index.len(), index.entries().len());
    }
}
