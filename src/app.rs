use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::collect::{BeesSignal, send_bees_signal};
use crate::model::{BeesInstanceSnapshot, BeesMonitorSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Counters,
    Threads,
    Logs,
}

impl Tab {
    pub const ALL: [Self; 4] = [Self::Overview, Self::Counters, Self::Threads, Self::Logs];

    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Counters => 1,
            Self::Threads => 2,
            Self::Logs => 3,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "1 Overview",
            Self::Counters => "2 Counters",
            Self::Threads => "3 Threads",
            Self::Logs => "4 Logs",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Counters,
            Self::Counters => Self::Threads,
            Self::Threads => Self::Logs,
            Self::Logs => Self::Overview,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Overview => Self::Logs,
            Self::Counters => Self::Overview,
            Self::Threads => Self::Counters,
            Self::Logs => Self::Threads,
        }
    }
}

pub enum KeyOutcome {
    None,
    RefreshNow,
    Quit,
}

pub struct AppState {
    pub snapshot: Option<BeesMonitorSnapshot>,
    pub selected_idx: usize,
    pub tab: Tab,
    pub scroll_counters: usize,
    pub scroll_threads: usize,
    pub scroll_logs: usize,
    pub logs_follow: bool,
    pub filter_active: bool,
    pub filter_query: String,
    pub confirm_signal: Option<BeesSignal>,
    pub flash: Option<(String, bool)>,
    pub interval_ms: u64,
}

impl AppState {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            snapshot: None,
            selected_idx: 0,
            tab: Tab::Overview,
            scroll_counters: 0,
            scroll_threads: 0,
            scroll_logs: 0,
            logs_follow: true,
            filter_active: false,
            filter_query: String::new(),
            confirm_signal: None,
            flash: None,
            interval_ms,
        }
    }

    pub fn update_snapshot(&mut self, snap: BeesMonitorSnapshot) {
        if !snap.instances.is_empty() && self.selected_idx >= snap.instances.len() {
            self.selected_idx = snap.instances.len() - 1;
        }
        self.snapshot = Some(snap);
    }

    pub fn active_instance(&self) -> Option<&BeesInstanceSnapshot> {
        self.snapshot.as_ref().and_then(|s| {
            s.instances
                .get(self.selected_idx)
                .or_else(|| s.instances.first())
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> KeyOutcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return KeyOutcome::Quit;
        }

        if let Some(sig) = self.confirm_signal {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_signal = None;
                    if let Some(pid) = self
                        .active_instance()
                        .and_then(|i| i.unit.as_ref()?.main_pid)
                    {
                        match send_bees_signal(pid, sig) {
                            Ok(()) => {
                                self.flash =
                                    Some((format!("Sent {} to PID {pid}", sig.label()), false));
                                return KeyOutcome::RefreshNow;
                            }
                            Err(e) => {
                                self.flash = Some((format!("Signal failed: {e}"), true));
                            }
                        }
                    } else {
                        self.flash = Some(("No active MainPID to signal.".to_string(), true));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_signal = None;
                    self.flash = Some(("Signal cancelled.".to_string(), false));
                }
                _ => {}
            }
            return KeyOutcome::None;
        }

        if self.filter_active {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.filter_active = false;
                }
                KeyCode::Backspace => {
                    self.filter_query.pop();
                    self.reset_scroll_for_tab();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.filter_query.push(c);
                    self.reset_scroll_for_tab();
                }
                _ => {}
            }
            return KeyOutcome::None;
        }

        match key.code {
            KeyCode::Char('q') => return KeyOutcome::Quit,
            KeyCode::Esc if !self.filter_query.is_empty() => {
                self.filter_query.clear();
                self.reset_scroll_for_tab();
            }
            KeyCode::Char('1') => self.tab = Tab::Overview,
            KeyCode::Char('2') => self.tab = Tab::Counters,
            KeyCode::Char('3') => self.tab = Tab::Threads,
            KeyCode::Char('4') => self.tab = Tab::Logs,
            KeyCode::Tab => self.tab = self.tab.next(),
            KeyCode::BackTab => self.tab = self.tab.prev(),
            KeyCode::Char('[') => {
                if let Some(snap) = &self.snapshot
                    && !snap.instances.is_empty()
                {
                    self.selected_idx = if self.selected_idx == 0 {
                        snap.instances.len() - 1
                    } else {
                        self.selected_idx - 1
                    };
                }
            }
            KeyCode::Char(']') => {
                if let Some(snap) = &self.snapshot
                    && !snap.instances.is_empty()
                {
                    self.selected_idx = (self.selected_idx + 1) % snap.instances.len();
                }
            }
            KeyCode::Char('r') => {
                self.flash = Some(("Refreshing…".to_string(), false));
                return KeyOutcome::RefreshNow;
            }
            KeyCode::Char('/') => {
                if self.tab != Tab::Overview {
                    self.filter_active = true;
                }
            }
            KeyCode::Char('f') if self.tab == Tab::Logs => {
                self.logs_follow = !self.logs_follow;
            }
            KeyCode::Char('p') => self.request_signal(BeesSignal::Pause),
            KeyCode::Char('u') => self.request_signal(BeesSignal::Resume),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-10),
            KeyCode::PageDown => self.scroll_by(10),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to_bottom(),
            _ => {}
        }

        KeyOutcome::None
    }

    fn request_signal(&mut self, sig: BeesSignal) {
        let Some(snap) = &self.snapshot else {
            return;
        };
        if !snap.is_root {
            self.flash = Some((
                "Run with sudo bees-tui to send signals to beesd.".to_string(),
                true,
            ));
            return;
        }
        if self
            .active_instance()
            .and_then(|i| i.unit.as_ref()?.main_pid)
            .is_some()
        {
            self.confirm_signal = Some(sig);
        } else {
            self.flash = Some(("No running bees MainPID found.".to_string(), true));
        }
    }

    fn reset_scroll_for_tab(&mut self) {
        match self.tab {
            Tab::Overview => {}
            Tab::Counters => self.scroll_counters = 0,
            Tab::Threads => self.scroll_threads = 0,
            Tab::Logs => self.scroll_logs = 0,
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let target = match self.tab {
            Tab::Overview => return,
            Tab::Counters => &mut self.scroll_counters,
            Tab::Threads => &mut self.scroll_threads,
            Tab::Logs => {
                if delta < 0 {
                    self.logs_follow = false;
                }
                &mut self.scroll_logs
            }
        };
        if delta < 0 {
            *target = target.saturating_sub(delta.unsigned_abs());
        } else {
            *target = target.saturating_add(delta as usize);
        }
    }

    fn scroll_to_top(&mut self) {
        match self.tab {
            Tab::Overview => {}
            Tab::Counters => self.scroll_counters = 0,
            Tab::Threads => self.scroll_threads = 0,
            Tab::Logs => {
                self.logs_follow = false;
                self.scroll_logs = 0;
            }
        }
    }

    fn scroll_to_bottom(&mut self) {
        match self.tab {
            Tab::Overview => {}
            Tab::Counters => self.scroll_counters = usize::MAX / 2,
            Tab::Threads => self.scroll_threads = usize::MAX / 2,
            Tab::Logs => {
                self.logs_follow = true;
            }
        }
    }
}
