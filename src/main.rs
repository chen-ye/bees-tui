mod app;
mod collect;
mod model;
mod parsers;
mod ui;

use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::app::{AppState, KeyOutcome};
use crate::collect::{BeesCollector, expand_tilde};
use crate::model::BeesMonitorSnapshot;

#[derive(Debug, Parser)]
#[command(
    name = "bees-tui",
    version,
    about = "Terminal monitor for the bees btrfs deduplication service"
)]
struct Cli {
    /// Polling interval in milliseconds
    #[arg(short, long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(250..=60000))]
    interval: u64,

    /// Filter by beesd unit name or filesystem UUID prefix
    #[arg(short, long)]
    unit: Option<String>,

    /// Path to bees configuration directory
    #[arg(short, long, default_value = "/etc/bees")]
    config_dir: String,
}

enum WorkerCmd {
    Refresh,
    Stop,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let config_dir = expand_tilde(&cli.config_dir);
    let interval = Duration::from_millis(cli.interval);

    let (snap_tx, snap_rx) = mpsc::channel::<BeesMonitorSnapshot>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();

    let unit_filter = cli.unit.clone();
    let worker = thread::spawn(move || {
        let mut collector = BeesCollector::new(config_dir, unit_filter);
        loop {
            let snap = collector.collect();
            if snap_tx.send(snap).is_err() {
                break;
            }
            match cmd_rx.recv_timeout(interval) {
                Ok(WorkerCmd::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Ok(WorkerCmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut terminal = ratatui::init();
    let mut app = AppState::new(cli.interval);
    let res = run_loop(&mut terminal, &mut app, &snap_rx, &cmd_tx);
    ratatui::restore();

    let _ = cmd_tx.send(WorkerCmd::Stop);
    let _ = worker.join();
    res
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut AppState,
    snap_rx: &mpsc::Receiver<BeesMonitorSnapshot>,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
) -> io::Result<()> {
    loop {
        while let Ok(snap) = snap_rx.try_recv() {
            app.update_snapshot(snap);
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match app.handle_key(key) {
                    KeyOutcome::Quit => return Ok(()),
                    KeyOutcome::RefreshNow => {
                        let _ = cmd_tx.send(WorkerCmd::Refresh);
                    }
                    KeyOutcome::None => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
