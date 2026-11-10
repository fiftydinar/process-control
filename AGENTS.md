# AGENTS.md

<!-- BEGIN SHARED GUIDELINES: copied from pkgforge-apps/.github by scripts/sync-guidelines.sh. Edit them there. -->

Guidelines for anyone, human or agent, working on a pkgforge-apps
repository. Every app in the organization starts from the
[app template](https://github.com/pkgforge-apps/app-template) and follows
these rules, so the repositories stay consistent with each other.

These guidelines, the shared skills, the shared scripts and the CI and
release workflows live in
[pkgforge-apps/.github](https://github.com/pkgforge-apps/.github). Each
app carries a copy that `scripts/sync-guidelines.sh` refreshes. When
they change there, pkgforge-apps-bot opens a pull request in every app
with the update. Edit them there,
never in an app; an edit made in an app is overwritten by the next sync.
`.agents/shared-guidelines.lock` lists every shared file.

## Purpose

pkgforge-apps builds lightweight native desktop apps: original ones,
and alternatives to heavyweight apps, Electron apps in particular. Each
app is a single Rust binary. Small binary, fast start, low memory: if a
change works against those, it needs a strong reason.

### Choosing the GUI library

- [egui](https://github.com/emilk/egui), through eframe, is the default
  and what the app template is built on. It is immediate mode and
  mature: a good fit for tools, utilities, viewers and dashboards.
- [smudge](https://github.com/QaidVoid/smudge) is designed for
  general-purpose apps: a retained widget tree, fine-grained reactive
  signals, and repainting only what changed, which keeps an idle app
  idle. Prefer it for conventional desktop apps with many views, forms
  or documents. It is still in development: its API changes without
  notice and it is not on crates.io yet, so check its state and agree on
  it with the maintainer before starting an app on it.

Pick one per app at the start; do not mix them.

## Platforms

- Every app runs on Linux, macOS and Windows unless its function is tied
  to one OS. Say so in the README when it is.
- Put OS-specific code behind `#[cfg(target_os = "...")]` in its own
  module, with a fallback or a clear "unsupported" path on the others.
- Never hard-code path separators, line endings or config locations.
  Use `std::path`, and the platform conventions for config, data and
  cache directories.
- CI tests on all three platforms. A change that only compiles on yours
  is not done.
- An app that links a system library names its Debian packages, one per
  line, in `.github/apt-packages.txt`, and the jobs that build on Ubuntu
  install them first. The jobs that build in an Arch container read the
  same list in `.github/pacman-packages.txt`, with Arch's package names.
  Both files take comments. Keep them to what the build needs: the
  release bundles carry their own libraries, so this is for compiling,
  not for running.

## Workflow

- Commit one logical change at a time. Never commit a broken state.
- Run `scripts/check.sh` before every commit. It runs what CI runs.
- Never bump the version or edit `CHANGELOG.md` by hand. release-plz
  owns both.

### Commit messages

Semantic, imperative, one line, at most 68 characters:

```
type: imperative message
type(scope): imperative message
```

Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `style`,
`chore`, `ci`, `revert`. Use `feat!:` or a `BREAKING CHANGE:` footer for
breaking changes. Keep any body short. The commit types decide the next
version and the changelog groups, so pick them carefully:

- `feat: add a recent files menu`
- `fix(editor): keep the cursor on undo`
- `perf: cache decoded thumbnails`

## Code

- Write idiomatic Rust, edition 2024, on the latest stable toolchain.
- Document every item: modules (`//!`), functions, structs, enums,
  variants, fields and constants (`///`). Doc comments say what a thing
  is for and any contract a caller must keep. CI enforces this through
  `missing_docs` and `clippy::missing_docs_in_private_items`.
- No narrative comments. Do not restate what the code does. An inline
  comment is for a non-obvious *why*: a platform quirk, a workaround, a
  constraint that is not visible from the code.
- ASCII only, in every file: source, docs, configs, scripts. Write
  glyphs in Rust strings as `\u{..}` escapes. `scripts/check-ascii.sh`
  enforces it.
- Do not over-engineer. No abstraction, trait, generic or config knob
  for a case that does not exist yet. Three similar lines beat a
  premature helper.
- Prefer `Result` with context over panics. `expect` is for invariants
  that cannot fail, with a message that states the invariant.
- Scripts are POSIX `sh` (`#!/bin/sh`), never bash. They must pass
  `shellcheck -s sh`.

### egui apps

- Keep state and logic apart from drawing. Logic lives in plain types
  with plain methods that tests can call; `ui` functions read that
  state and turn input into calls on it.
- Never block the UI thread. Run I/O and heavy work on a
  `std::thread`, send results back over a channel, and call
  `ctx.request_repaint()` when they arrive. Reach for an async runtime
  only when the work is genuinely async-heavy.
- Repaint on demand. Do not request continuous repaints unless
  something is animating.
- The renderer is glow (OpenGL). Keep eframe's default features off and
  enable only what the app uses.
- Keep the `--smoke` flag working. CI and the release pipeline launch
  every build with it to prove the app opens a window and draws.

### smudge apps

- The template is built on egui. A smudge app starts from it and
  replaces `src/` and the eframe dependency; everything else stays,
  including the `--smoke` flag, which must draw a frame and exit 0.
- Until smudge is on crates.io, depend on it by git, pinned to a commit
  with `rev`. Move the pin deliberately, one change at a time, and read
  smudge's commits since the last pin first, since its API breaks
  without notice. Switch to crates.io once it is published.
- Follow smudge's own patterns: state in signals, views derived from
  them, and no work on the UI thread that could block a frame.
- Test views with smudge's headless harness, which runs the same frame
  pipeline a window does.

## Dependencies

- Add and change dependencies with `cargo add` and `cargo remove`,
  never by editing `Cargo.toml` by hand. Use `--no-default-features`
  and list only the features the app uses.
- Vet a crate before adding it. The `add-dependency` skill has the
  procedure. In short:
  - The latest release or commit must be under a year old.
  - An older crate is acceptable only when it is clearly complete,
    with no unanswered bug reports or stale PRs piling up.
  - Check what it pulls in with `cargo tree`. A large tree for a small
    feature is a reason to say no.
- Prefer inlining over a dependency when the functionality is a few
  dozen lines. A small, tested function in the app beats a crate.
- Prefer pure-Rust crates. A crate that links a C library adds a system
  dependency on every platform.
- No git dependencies and no `[patch]` sections. Releases build from
  crates.io. The one exception is smudge, until it is published.

## Tests

- Test behavior: what a user or caller can observe. A test should fail
  when the behavior breaks and pass through any refactor that keeps it.
- Test logic in unit tests next to the code. Test UI interaction when
  the behavior lives in the UI (clicking, typing, focus, layout that
  matters): with `egui_kittest` in an egui app, with smudge's headless
  harness in a smudge app.
- Every bug fix comes with a test that fails without the fix.
- Do not write tests that only restate the code: asserting a constant,
  that a constructor sets fields, or that a mock was called.
- Tests run on all three platforms. Use temporary directories, never
  fixed paths, and never depend on test order or wall-clock timing.

## Releases

Releases are fully automated by
[release-plz](https://release-plz.dev) on `GITHUB_TOKEN` only. No
personal tokens and no registry secrets; the `release` skill explains
the pipeline. In short:

1. Every push to `main` refreshes a release PR owned by
   github-actions[bot], with the next version and changelog.
2. Merging it tags the version and creates the GitHub release as a
   draft.
3. That starts `release.yml`, which builds and attaches the assets, then
   publishes the release. The organization makes published releases
   immutable, so assets go on while it is still a draft:

| Platform | Asset |
|----------|-------|
| Linux, portable | `<app>-<arch>-linux.onelf` |
| Linux, AppImage, opt-in | `<app>-anylinux-<arch>.AppImage` and its `.zsync` |
| Linux | `<app>-<arch>-linux.tar.xz` |
| macOS | `<app>-<arch>-macos.dmg` with an `.app` bundle |
| Windows | `<app>-x86_64-windows.exe` |

Only the plain Linux build is an archive, since it carries a desktop
entry and icons. The macOS disk image opens to the app and an
Applications link, the Windows executable runs as is with the C runtime
linked in, and the `.onelf` and `.AppImage` are single files. No
checksum files are published; every asset carries a build provenance
attestation instead.

The onelf bundle uses the stipplefs payload. An egui app's bundle
carries Mesa, so it draws on hosts without a usable GL stack, musl
distributions included; NVIDIA's driver still comes from the host. It
is the Mesa [onelf-pkgs/mesa](https://github.com/onelf-pkgs/mesa) builds
without LLVM, which adds about 8 MB; do not drop it to save space. A smudge
app drawing with its CPU renderer needs no GL, so it turns `gl` and
`dri` off in `onelf.toml`; confirm with the `test-linux-bundle` skill.

The onelf bundle updates itself with `--onelf-update`, downloading only
what changed since the copy it replaces. Once the app has its update key
it accepts only releases signed with it; the `release` skill covers how
the key is made and used.

Run the `test-linux-bundle` skill before merging any change that can
alter what the bundle loads: `onelf.toml`, a dependency that links or
opens a system library, an onelf upgrade, or a feature that reaches a
new part of the system such as audio, file dialogs or notifications.
The `onelf` skill covers inspecting and debugging a bundle.

An app may also ship an Anylinux AppImage, for users and managers that
expect that format, by setting `appimage = true` under
`[package.metadata.pkgforge]` in `Cargo.toml`. It is a second format,
not a replacement for the onelf bundle. The release pipeline builds it
with `scripts/package-appimage.sh`; the `anylinux-appimage` skill
covers it. An app whose stack quick-sharun cannot detect, such as the
audio servers reached through ALSA plugins, names the `DEPLOY_*`
variables it needs in `packaging/appimage/deploy.env`.

Apps are not published to crates.io by default. When one should be,
publish it through crates.io trusted publishing, never with a token
stored in repository secrets.

## Docs

- The docs site lives in `docs/`, built with VitePress and bun, and
  deploys to GitHub Pages from `main`.
- The site is personalized for the app: its own palette, type and
  home page. Do not ship the stock VitePress look. The `app-docs` skill
  covers it.
- Update the docs in the same change as the behavior they describe.

## Icons

`assets/icon.svg` is the single source for every icon, or
`assets/icon.png` when the icon is pixel art. Regenerate the rasters, the
Windows `.ico` and the docs favicons with `scripts/icons.sh` after
changing it. The `app-icon` skill covers the design rules.

## Layout

```
.
|-- src/
|   |-- main.rs          entry point, window options
|   `-- app.rs           application state and root UI
|-- assets/
|   |-- icon.svg         icon source, or icon.png for pixel art
|   |-- icon.ico         Windows icon, generated
|   `-- icons/           PNG sizes, generated
|-- packaging/
|   |-- linux/app.desktop
|   |-- appimage/deploy.env  optional DEPLOY_* variables
|   `-- macos/Info.plist
|-- docs/                VitePress site
|-- scripts/             shared, except bootstrap.sh in the template
|   |-- check.sh         every CI gate
|   |-- check-ascii.sh   the ASCII rule
|   |-- icons.sh         icon generation
|   |-- package.sh       Linux archive, macOS disk image, Windows exe
|   |-- package-onelf.sh portable Linux bundle
|   `-- sync-guidelines.sh  refresh every shared file
|-- onelf.toml           onelf recipe
|-- .release-plz.toml
|-- .agents/skills/      shared and app-specific skills
|-- .claude/skills/      a symlink per skill, for Claude Code
`-- .github/workflows/   shared callers: ci, release-plz, release,
                         docs
```

## Skills

Procedures live in `.agents/skills/`. Claude Code only looks in
`.claude/skills/`, so each skill also has a symlink there, which
`scripts/sync-guidelines.sh` creates. These are shared by every app:

- `add-dependency` vets and adds a crate, or decides to inline it.
- `anylinux-appimage` packages the app as an Anylinux AppImage.
- `app-icon` designs the icon and regenerates every raster.
- `app-docs` personalizes the docs site.
- `release` explains and troubleshoots the release pipeline, including
  crates.io trusted publishing.
- `onelf` inspects and debugs the portable Linux bundle.
- `test-linux-bundle` smoke-runs a bundle on the hosts users have.
- `sync-guidelines` refreshes the shared guidelines and skills.

The template also carries `bootstrap-app`, which turns a fresh copy into
a new app and is removed once that is done. An app may add skills of its
own; give them names the shared ones do not use, since a sync replaces a
shared skill of the same name.

<!-- END SHARED GUIDELINES -->

## This app

Process Control is a task manager: it shows the machine's processes and the
applications they belong to, with live graphs, and sends signals to them. It is
an alternative to Mission Center, GNOME's System Monitor and the other GTK task
managers, and it exists because those are slow to open and freeze under load,
which is exactly when a task manager is opened.

Linux only, and it builds for no other system: `src/platform/mod.rs` fails the
build elsewhere with a message that says why, rather than carrying a stub of a
window that could show nothing. Adding a platform means a second reader behind
that module, not a change to the app. The shared guidelines' rule about all
three platforms and the shared CI and release matrices that enforce it do not
fit an app like this; see the "One OS only" note below.

### Layout, as this app actually is

The shape above is the organization's; this app has no `build.rs` and no
`packaging/macos`, since nothing here builds for Windows or macOS.
`scripts/package.sh` only makes the Linux archive, and `scripts/icons.sh`
writes no Windows `.ico`. `src/desktop.rs` is the app's own, and
`src/platform/linux/` holds the readers.

### Reading the system

- Everything in `src/platform/linux/` is a read of `/proc` or `/sys`, and none
  of it may run on the drawing thread. `src/sampler.rs` owns the thread that
  reads at the configured interval and sends each `Sample` over a channel;
  `src/platform/linux/mod.rs`'s `Reader` is what turns two reads into rates.
  The `Sample` is a value: drawing never waits on the system.
- `process.rs` reads `/proc/<pid>` and the Flatpak sandbox id a process runs
  under, `system.rs` the machine-wide counters, `net.rs` the TCP byte counters
  (`NETLINK_SOCK_DIAG`, matched to a process by the socket inodes in its `fd`
  directory), `gpu.rs` the kernel's DRM engine accounting, and `windows.rs` the
  windows on the X display.
- The per-process network column only counts TCP. A program that streams over
  UDP shows nothing there, because the kernel keeps no byte counter for it.

### Applications

`src/apps.rs` turns a `Sample` into the rows. A process belongs to an
application when it, or an ancestor, owns a window: `_NET_WM_PID` says which
process owns it and `WM_CLASS` says which program it is. A Wayland session
publishes neither, to anyone but the compositor, so when the sample holds no
windows at all `Applications::new` falls back to matching each process's
executable against the desktop entries. That fallback is what gives a Wayland
row its name and icon; do not remove it as dead code on a machine with X. `src/desktop.rs`
indexes the desktop entries and matches `StartupWMClass` against
both strings of `WM_CLASS`, so an application is recognised whatever packaged
it. The index also reads the desktop files Flatpak exports under its user and
system installations, which are not part of the host's `XDG_DATA_DIRS`. A
Flatpak process runs the app's own binary rather than the `flatpak` command its
entry names, so the sandbox id from `/.flatpak-info` and `FLATPAK_ID` is what
finds the entry in the executable fallback; `bwrap` and `xdg-dbus-proxy` share
the info file but not `FLATPAK_ID`, so they are not applications of their own.
All of this reads files Flatpak installs; it consults no Flatpak, Snap or
systemd API or database, which do not exist on a great many systems.

`src/desktop.rs` is at the crate root rather than behind the platform module
because it is plain XDG file parsing: it reads the same `.desktop` files for any
app, and nothing in it is a Linux syscall.

`src/icons.rs` finds and decodes the icons the entries name, off the drawing
thread, including the themes Flatpak exports for its apps. Adding a dependency
that reads a new image format belongs there.

### One OS only

The shared guidelines ask every app to build on Linux, macOS and Windows. This
one cannot: a task manager is a reader of the process table, and this reads
Linux's. The shared workflows assume the three platforms, so this app carries
Linux-only replacements instead of calling them:

- `.github/workflows/ci.yml` keeps the shared lint, test, smoke and docs jobs
  but runs them on `ubuntu-latest` alone, with no macOS or Windows matrix.
- `.github/workflows/release.yml` builds the Linux archive, onelf bundle and
  AppImage on GitHub, then publishes the draft release. It keeps the name and
  `tag` input release-plz dispatches, so a release still starts by itself.

Do not paper over the missing platforms by adding a stub platform back.

### Things to know

- The window has two views of one sample: applications, and the process tree.
  An application's subtree starts folded, which is why `Row::has_children`
  exists: a folded row draws no children, so the arrow could not be worked out
  from the rows alone.
- `--smoke` must keep working, on every platform. CI launches it under Xvfb to
  prove a build opens a window and draws.
- `src/preview.rs` draws one frame into a PNG with tiny-skia, so a change can
  be looked at on a machine with no display server. It is `#[cfg(test)]` only,
  and the docs' screenshots come from it: run
  `cargo test write_the_screenshots -- --ignored`.

### Testing

The platform modules have tests that read the live system, so they are skipped
nowhere and skipped by nothing: they assert what is true of any machine the app
runs on (that the process table holds the test itself, that the CPU counters
moved at some point) rather than exact numbers. `src/ui.rs` draws through
`egui_kittest`, which needs no display.
