# bees-tui

Terminal monitor for [`bees`](https://github.com/Zygo/bees) (`beesd@<uuid>.service`) btrfs deduplication daemons, built with Rust and [Ratatui](https://ratatui.rs).

## Why run with `sudo`?

Systemd's default `beesd@.service` sets `RuntimeDirectoryMode=0700` on `/run/bees`, and `journalctl` / `/proc/<pid>/io` require root or `systemd-journal`.

- **With `sudo`**: reads live `$BEESSTATUS` (`/run/bees/<uuid>.status` — scan `PROGRESS`, `TOTAL`/`RATES`, `THREADS`), `$BEESHOME/beesstats.txt` (hash table fill & cell breakdown), `$BEESHOME/beescrawl.dat`, `/proc/<pid>/io` read/write rates, `journalctl` logs, and allows `SIGUSR1`/`SIGUSR2` worker pause/resume.
- **Unprivileged**: falls back cleanly to `systemctl`, `/etc/bees/*.conf`, `/proc/<pid>/stat`, `/proc/loadavg`, and `btrfs filesystem usage`.

## Install

```sh
cargo install --git https://github.com/chen-ye/bees-tui --locked
```

Or from a local checkout:

```sh
cargo install --path . --locked
```

Because `sudo` often uses a restricted `PATH` (`secure_path`), invoke the installed binary via:

```sh
sudo ~/.cargo/bin/bees-tui
# or install system-wide:
sudo install -m 0755 ~/.cargo/bin/bees-tui /usr/local/bin/bees-tui
sudo bees-tui
```

## Controls

| Key | Action |
|---|---|
| `1`–`4`, `Tab` | Switch tabs (`Overview`, `Counters`, `Threads`, `Logs`) |
| `[` / `]` | Switch between `beesd` instances |
| `↑` / `↓` / `j` / `k` / `PgUp` / `PgDn` | Scroll active table or log view |
| `/` | Filter counters, threads, or logs |
| `f` | Toggle log auto-follow (`Logs` tab) |
| `r` | Force immediate refresh |
| `p` / `u` | Pause (`SIGUSR1`) / Resume (`SIGUSR2`) workers (with confirmation) |
| `q` / `Ctrl+C` | Quit |
