<p align="center">
  <img src="assets/icon.svg" width="128" alt="">
</p>

<h1 align="center">Process Control</h1>

<p align="center">A fast, light task manager with applications, not just processes</p>

<p align="center">
  <a href="https://pkgforge-apps.github.io/process-control/">Documentation</a> |
  <a href="https://github.com/pkgforge-apps/process-control/releases/latest">Latest release</a>
</p>

Process Control shows what your machine is doing right now and lets you act on
it. It reads the process table itself, groups processes into the applications
that own the windows, and starts at once. There is no helper daemon, no
runtime to install, and nothing to configure before the window is useful.

It is an alternative to Mission Center, GNOME's System Monitor and the other
GTK task managers, which take a moment to open and can freeze under load. A
task manager is opened exactly when the machine is busy, so being quick and
staying responsive is the whole point: this is one native Rust binary drawn
with egui, and the process table is read on a thread of its own, so the window
never waits on it.

## Features

- **Applications, not just processes.** The list is split into an
  **Applications** section, one row per program under its own name and icon
  with its helper processes counted in its totals, and a **Processes** section
  for everything that belongs to no program. Each heading carries its section's
  totals and folds away. The window class and the process that owns the window are read
  from the display server, then matched against the desktop entries, so this
  works for any packaging format: Flatpak, Snap, `.deb`, or a binary in your
  home directory. Nothing here depends on Flatpak, Snap or systemd.
- **A process tree.** Every process under its parent, with an application's
  tree folded away until you ask for it. Kernel threads stay out of the way
  unless you turn them on.
- **The numbers, per process and for the machine.** CPU per core and in
  total, memory and swap, GPU usage, network and storage traffic, all with a
  history graph across the top of the window and a column per row.
  CPU and storage come from `/proc`, the GPU from the kernel's own DRM
  accounting, and the per-process network figures from the sockets each
  process holds.
- **Act on a process.** Terminate it (SIGTERM), force stop it (SIGKILL, with
  a question first), send any signal from the menu, or open its details:
  command line, executable, working directory, user, memory, I/O, open files,
  OOM score and window class.
- **Built to stay light.** The table draws only the rows that are on screen,
  the icons are decoded off the drawing thread, and the window repaints only
  when a sample arrives or you touch it.

## Platforms

Linux only: it reads `/proc` and `/sys`, and asks the X display which window
each process owns. It does not build for other systems.

On an X11 session that is what groups processes into applications. A Wayland
compositor does not tell any other program which process owns a window, so on
a Wayland session there is nothing to ask: there, applications are recognised
by the program each process runs, matched against the desktop entries. That
gives the same names and icons; it is the window class that a Wayland session
cannot supply.

## Install

Download the file for your platform from the
[latest release](https://github.com/pkgforge-apps/process-control/releases/latest).

| Platform | File |
|----------|------|
| Linux, portable | `process-control-<arch>-linux.onelf` |
| Linux, AppImage | `process-control-anylinux-<arch>.AppImage` |
| Linux | `process-control-<arch>-linux.tar.xz` |

The `.onelf` file is a single executable that carries its own libraries.
Make it executable and run it:

```sh
chmod +x process-control-x86_64-linux.onelf
./process-control-x86_64-linux.onelf
```

Run it once with `--onelf-integrate` to add it to your application menu,
and update it in place with `--onelf-update`, which downloads only what
changed.

<!-- AppImage: keep this section, and the table row above, only when
Cargo.toml sets appimage = true under [package.metadata.pkgforge]. -->
The `.AppImage` file is the same app as an
[Anylinux AppImage](https://github.com/pkgforge-dev/Anylinux-AppImages),
for AppImage managers such as [AM](https://github.com/ivan-hc/AM) or
[soar](https://github.com/pkgforge/soar). It runs the same way and
updates itself:

```sh
chmod +x process-control-anylinux-x86_64.AppImage
./process-control-anylinux-x86_64.AppImage
```
<!-- /AppImage -->

## Build from source

```sh
cargo build --release --locked
./target/release/process-control
```

## License

[MIT](LICENSE)
