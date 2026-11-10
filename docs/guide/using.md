# Using it

## The graphs

The strip across the top of the window shows the machine as a whole: CPU,
memory, GPU, network and storage. Each card carries the current reading, a
short explanation beside it and two minutes of history behind it.

- **CPU** is a percentage of every core together, with the busiest core's share
  beside it.
- **Memory** is what is in use of what the machine has, with swap beside it.
- **GPU** is the kernel's own count from the graphics driver. It reads `--` on a
  machine whose driver does not report one, which is every NVIDIA setup and any
  kernel older than 6.4.
- **Network** counts every interface but the loopback, so a local copy does not
  show up as network use.
- **Storage** counts every whole disk and leaves out the loopback devices and
  the RAM disks, so a file is not counted twice.

## Applications and processes

The tool bar switches between two views of the same sample.

**Applications** groups processes into the programs they belong to, in two
sections. **Applications** holds one row per program, under its own name and
icon, with the processes it started counted in its totals rather than listed:
the arrow beside a program opens it. **Processes** holds everything that belongs
to no program, keeping its own tree, so a daemon and the helper it started stay
a parent and a child.

Each section heading carries the totals of what it holds and folds away like a
row, so a closed section still says how much of the machine it accounts for.
Sorting a column orders the rows within a section and leaves the headings
where they are.

**Process Tree** puts every process under its parent. An application's tree is
folded away by default: the arrow beside a row opens it, and the processes
inside keep their own names so you can see which part of a program is doing the
work. Kernel threads are left out unless **View > Show Kernel Threads** turns
them on.

Click a row to select it. Ctrl-click adds a row to the selection, shift-click
extends it, and a right click opens the actions for the row under the pointer.
Selecting an application selects every process it stands for, which is what
makes a stop from that view stop the whole program.

## Acting on a process

**Terminate** sends `SIGTERM`, which asks the process to finish what it is
doing and exit. **Force Stop** sends `SIGKILL`, which the kernel obeys at once
and the process cannot clean up after; that is why it asks first. **Signal**
offers every other one, from `SIGHUP` to `SIGSTOP` and `SIGCONT`.

A process that belongs to another user answers with a permission error, which
the status line shows as it is. The app does not ask for more rights than you
have, so run it as that user if you need to stop their processes.

## The details panel

Double-click a row, or use **Details**, to open the panel on the right. It
follows its process: the numbers in it are as fresh as the table's, and a
process that exits closes it with a note in the status line.

The panel shows the command line, the executable, the working directory, the
user, the state, the memory and swap, the storage the process read and wrote,
how many files it has open, its out-of-memory score, how long it has been
running, and the window class of its window. The window class is the name a
desktop entry's `StartupWMClass` carries, which is what groups a program's
processes together; it is worth knowing when a program shows up under a name
you did not expect.

## Settings

Everything is in the **View** menu: which view is shown, whether kernel threads
are listed, whether icons are drawn, the colour theme and how often the table is
read. Settings are kept between runs in
`$XDG_CONFIG_HOME/process-control/config.toml`.

**Refresh Every** decides how often the process table is read. Once a second is
the default and what the other task managers do; half a second makes the graphs
smoother at the cost of more reading, and five seconds is enough for watching
memory creep up on a quiet machine. **File > Refresh Now** reads at once,
whatever the interval says.
