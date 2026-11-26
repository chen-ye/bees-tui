use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Sparkline, Table, Tabs, Wrap,
};

use crate::app::{AppState, Tab};
use crate::model::{
    BeesInstanceSnapshot, BeesMonitorSnapshot, FileAccess, JournalAccess, JournalSeverity,
};
use crate::parsers::{
    format_bytes, format_duration_secs, format_rate_bytes, short_uuid, unicode_bar,
};

pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header + tabs
            Constraint::Min(8),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    render_header(frame, chunks[0], app);

    match app.snapshot.as_ref() {
        None => {
            let loading = Paragraph::new("Collecting bees daemon status…")
                .block(Block::default().borders(Borders::ALL).title(" bees-tui "));
            frame.render_widget(loading, chunks[1]);
        }
        Some(snap) if snap.instances.is_empty() => {
            let msg = Paragraph::new(
                "No beesd@*.service units or /etc/bees/*.conf files detected.\n\nPress [r] to refresh or [q] to quit.",
            )
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" No bees service found "));
            frame.render_widget(msg, chunks[1]);
        }
        Some(snap) => {
            let inst = &snap.instances[app.selected_idx.min(snap.instances.len() - 1)];
            match app.tab {
                Tab::Overview => render_overview(frame, chunks[1], snap, inst),
                Tab::Counters => render_counters(
                    frame,
                    chunks[1],
                    &mut app.scroll_counters,
                    &app.filter_query,
                    inst,
                ),
                Tab::Threads => render_threads(
                    frame,
                    chunks[1],
                    &mut app.scroll_threads,
                    &app.filter_query,
                    inst,
                ),
                Tab::Logs => render_logs(
                    frame,
                    chunks[1],
                    &mut app.scroll_logs,
                    app.logs_follow,
                    &app.filter_query,
                    inst,
                ),
            }
        }
    }

    render_footer(frame, chunks[2], app);

    if let Some(sig) = app.confirm_signal {
        render_confirm_modal(frame, area, app, sig.label());
    }
}

fn unit_status_style(inst: &BeesInstanceSnapshot) -> (&'static str, Style) {
    let Some(u) = &inst.unit else {
        return ("unloaded", Style::default().fg(Color::Yellow));
    };
    match (u.active_state.as_str(), u.sub_state.as_str()) {
        ("active", "running") => (
            "running",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        ("failed", _) | (_, "failed") => (
            "failed",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        ("activating", _) | ("reloading", _) => ("starting", Style::default().fg(Color::Cyan)),
        _ => ("stopped", Style::default().fg(Color::Yellow)),
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    let mut spans = vec![
        Span::styled(
            " bees-tui ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];

    if let Some(snap) = &app.snapshot {
        if let Some(inst) = snap
            .instances
            .get(app.selected_idx.min(snap.instances.len().saturating_sub(1)))
        {
            let (st_label, st_style) = unit_status_style(inst);
            spans.push(Span::styled(
                format!("beesd@{}", short_uuid(&inst.uuid)),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if snap.instances.len() > 1 {
                spans.push(Span::styled(
                    format!(" ({}/{})", app.selected_idx + 1, snap.instances.len()),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            spans.push(Span::raw(" ["));
            spans.push(Span::styled(st_label, st_style));
            spans.push(Span::raw("] "));

            if let Some(btrfs) = &inst.btrfs {
                spans.push(Span::styled(
                    btrfs.mount_point.clone(),
                    Style::default().fg(Color::Cyan),
                ));
                spans.push(Span::raw(" · "));
            }
            if let Some(pid) = inst.unit.as_ref().and_then(|u| u.main_pid) {
                spans.push(Span::raw(format!("PID {pid} · ")));
            }
            if let FileAccess::Ok { data, .. } = &inst.stats_file
                && let Some(ver) = &data.version
            {
                spans.push(Span::styled(
                    format!("bees {ver} · "),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }

        if snap.is_root {
            spans.push(Span::styled("root", Style::default().fg(Color::Green)));
        } else {
            spans.push(Span::styled(
                "unprivileged",
                Style::default().fg(Color::Yellow),
            ));
        }
        spans.push(Span::styled(
            format!(" · {} ({}ms)", snap.collected_at_hms, app.interval_ms),
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), rows[0]);

    let titles: Vec<Line<'_>> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .block(Block::default().borders(Borders::BOTTOM))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, rows[1]);
}

fn render_overview(
    frame: &mut Frame<'_>,
    area: Rect,
    snap: &BeesMonitorSnapshot,
    inst: &BeesInstanceSnapshot,
) {
    let main_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),  // 3 summary cards
            Constraint::Length(10), // scan progress
            Constraint::Min(6),     // workers + logs
        ])
        .split(area);

    let top_cols = if area.width >= 96 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(main_rows[0])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ])
            .split(main_rows[0])
    };

    render_daemon_card(frame, top_cols[0], inst);
    render_load_hash_card(frame, top_cols[1], snap, inst);
    render_btrfs_dedupe_card(frame, top_cols[2], inst);

    render_progress_panel(frame, main_rows[1], snap.is_root, inst);

    let bottom_cols = if area.width >= 96 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(main_rows[2])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(main_rows[2])
    };

    render_workers_preview(frame, bottom_cols[0], inst);
    render_journal_preview(frame, bottom_cols[1], inst);
}

fn render_daemon_card(frame: &mut Frame<'_>, area: Rect, inst: &BeesInstanceSnapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Daemon & I/O ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cpu_str = inst
        .cpu_percent
        .map(|c| format!("{c:.1}%"))
        .unwrap_or_else(|| "—".to_string());
    let threads_str = inst
        .proc_sample
        .as_ref()
        .map(|p| p.threads.to_string())
        .or_else(|| {
            inst.unit
                .as_ref()
                .and_then(|u| u.tasks_current)
                .map(|t| t.to_string())
        })
        .unwrap_or_else(|| "—".to_string());
    let rss_str = format_bytes(
        inst.proc_sample
            .as_ref()
            .and_then(|p| p.vm_rss_bytes)
            .or_else(|| inst.unit.as_ref().and_then(|u| u.memory_current_bytes)),
    );
    let db_str = format_bytes(inst.config.as_ref().and_then(|c| c.db_size_bytes));
    let up_str = format_duration_secs(inst.uptime_secs);
    let io_str = match (inst.io_read_bps, inst.io_write_bps) {
        (Some(r), Some(w)) => format!(
            "R {} · W {}",
            format_rate_bytes(Some(r)),
            format_rate_bytes(Some(w))
        ),
        _ => {
            if matches!(inst.status_file, FileAccess::Denied { .. }) {
                "requires sudo".to_string()
            } else {
                "—".to_string()
            }
        }
    };

    if inner.height >= 6 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(1)])
            .split(inner);

        let lines = vec![
            Line::from(vec![
                Span::styled("CPU ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    cpu_str,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("   Thr {threads_str}   Up {up_str}")),
            ]),
            Line::from(vec![
                Span::styled("RSS ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{rss_str} / DB {db_str}")),
            ]),
            Line::from(vec![
                Span::styled("Disk ", Style::default().fg(Color::DarkGray)),
                Span::raw(io_str),
            ]),
            Line::from(vec![
                Span::styled("Opts ", Style::default().fg(Color::DarkGray)),
                Span::raw(
                    inst.config
                        .as_ref()
                        .map(|c| c.options.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("default"),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines), rows[0]);

        if !inst.cpu_history.is_empty() {
            let spark = Sparkline::default()
                .data(&inst.cpu_history)
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(spark, rows[1]);
        }
    } else {
        let text = format!("CPU {cpu_str} · Thr {threads_str} · RSS {rss_str} · Up {up_str}");
        frame.render_widget(Paragraph::new(text), inner);
    }
}

fn render_load_hash_card(
    frame: &mut Frame<'_>,
    area: Rect,
    snap: &BeesMonitorSnapshot,
    inst: &BeesInstanceSnapshot,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Load & Hash Table ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let target = inst.config.as_ref().and_then(|c| c.loadavg_target);
    let load_line = match (&snap.host_load, target) {
        (Some(l), Some(t)) => format!(
            "Load {:.2} / {t:.0} (5m {:.2}, 15m {:.2})",
            l.load1, l.load5, l.load15
        ),
        (Some(l), None) => format!("Load {:.2} / {:.2} / {:.2}", l.load1, l.load5, l.load15),
        (None, _) => "Load —".to_string(),
    };
    let load_pct = match (&snap.host_load, target) {
        (Some(l), Some(t)) if t > 0.0 => ((l.load1 / t) * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    };
    let load_color = match (&snap.host_load, target) {
        (Some(l), Some(t)) if l.load1 > t * 1.1 => Color::Yellow,
        _ => Color::Blue,
    };

    if inner.height >= 6 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        frame.render_widget(Paragraph::new(load_line), rows[0]);

        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(load_color))
            .percent(load_pct.round() as u16)
            .label(if target.is_some() {
                format!("{load_pct:.0}% of target")
            } else {
                "no target".to_string()
            });
        frame.render_widget(gauge, rows[1]);

        let hash_lines = match &inst.stats_file {
            FileAccess::Ok { data, .. } => {
                let occ = data
                    .occupied_percent
                    .map(|p| format!("{p:.1}%"))
                    .unwrap_or_else(|| "—".to_string());
                let cells = match (data.occupied_cells, data.total_cells) {
                    (Some(o), Some(t)) => format!("{o}/{t}"),
                    _ => "—".to_string(),
                };
                let breakdown = format!(
                    "comp {:.0}% · uncomp {:.0}% · toxic {:.1}%",
                    data.compressed_percent.unwrap_or(0.0),
                    data.uncompressed_percent.unwrap_or(0.0),
                    data.toxic_percent.unwrap_or(0.0),
                );
                vec![
                    Line::from(vec![
                        Span::styled("Hash ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            occ,
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!(" ({cells})")),
                    ]),
                    Line::from(Span::styled(
                        breakdown,
                        Style::default().fg(Color::DarkGray),
                    )),
                ]
            }
            FileAccess::Denied { .. } => vec![Line::from(Span::styled(
                "Hash fill: requires sudo (.beeshome)",
                Style::default().fg(Color::Yellow),
            ))],
            FileAccess::Missing { .. } => vec![Line::from(Span::styled(
                "beesstats.txt pending (hourly)",
                Style::default().fg(Color::DarkGray),
            ))],
            FileAccess::Error { error, .. } => vec![Line::from(Span::styled(
                format!("Hash err: {error}"),
                Style::default().fg(Color::Red),
            ))],
        };
        frame.render_widget(Paragraph::new(hash_lines), rows[2]);

        if !inst.load_history.is_empty() {
            let spark = Sparkline::default()
                .data(&inst.load_history)
                .style(Style::default().fg(load_color));
            frame.render_widget(spark, rows[3]);
        }
    } else {
        frame.render_widget(Paragraph::new(load_line), inner);
    }
}

fn render_btrfs_dedupe_card(frame: &mut Frame<'_>, area: Rect, inst: &BeesInstanceSnapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Btrfs & Dedupe ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let btrfs_pct = inst
        .btrfs
        .as_ref()
        .and_then(|b| b.data_used_percent)
        .unwrap_or(0.0);
    let btrfs_label = if let Some(b) = &inst.btrfs {
        format!(
            "Data {:.1}% ({} / {}) · Free {}",
            b.data_used_percent.unwrap_or(0.0),
            format_bytes(b.data_used_bytes),
            format_bytes(b.data_size_bytes),
            format_bytes(b.free_estimated_bytes),
        )
    } else {
        "No mounted btrfs filesystem".to_string()
    };

    let dedupe_lines = match &inst.status_file {
        FileAccess::Ok { data, .. } => {
            let find_total = |k: &str| data.total.iter().find(|(tk, _)| tk == k).map(|(_, v)| *v);
            let find_rate = |k: &str| data.rates.iter().find(|(rk, _)| rk == k).map(|(_, v)| *v);
            let dedup_b = find_total("dedup_bytes").unwrap_or(0);
            let dedup_r = find_rate("dedup_bytes");
            let read_b = find_total("block_bytes").unwrap_or(0);
            let read_r = find_rate("block_bytes");
            let hits = find_total("dedup_hit").unwrap_or(0);
            let miss = find_total("dedup_miss").unwrap_or(0);
            let bugs: u64 = data
                .total
                .iter()
                .filter(|(k, v)| k.starts_with("bug_") && *v > 0)
                .map(|(_, v)| *v)
                .sum();
            let throttled = find_total("crawl_throttled").unwrap_or(0);
            vec![
                Line::from(vec![
                    Span::styled("Deduped ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!(
                            "{} ({})",
                            format_bytes(Some(dedup_b)),
                            format_rate_bytes(dedup_r)
                        ),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Hashed  ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!(
                        "{} ({})",
                        format_bytes(Some(read_b)),
                        format_rate_bytes(read_r)
                    )),
                ]),
                Line::from(vec![
                    Span::styled("Hit/Ms  ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{hits} / {miss} · thr {throttled} · bugs {bugs}")),
                ]),
            ]
        }
        FileAccess::Denied { path, .. } => vec![
            Line::from(Span::styled(
                "Status file root-only (0700):",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::styled(
                path.clone(),
                Style::default().fg(Color::DarkGray),
            )),
        ],
        FileAccess::Missing { path } => vec![Line::from(Span::styled(
            format!("Missing: {path}"),
            Style::default().fg(Color::Yellow),
        ))],
        FileAccess::Error { error, .. } => vec![Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        ))],
    };

    if inner.height >= 6 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        frame.render_widget(Paragraph::new(btrfs_label), rows[0]);

        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(if btrfs_pct >= 90.0 {
                Color::Yellow
            } else {
                Color::Cyan
            }))
            .percent(btrfs_pct.clamp(0.0, 100.0).round() as u16)
            .label(format!("{btrfs_pct:.1}% data"));
        frame.render_widget(gauge, rows[1]);

        if let Some(b) = &inst.btrfs {
            frame.render_widget(
                Paragraph::new(format!(
                    "Meta {:.1}% ({} / {})",
                    b.metadata_used_percent.unwrap_or(0.0),
                    format_bytes(b.metadata_used_bytes),
                    format_bytes(b.metadata_size_bytes),
                )),
                rows[2],
            );
        }

        frame.render_widget(Paragraph::new(dedupe_lines), rows[3]);
    } else {
        frame.render_widget(Paragraph::new(btrfs_label), inner);
    }
}

fn render_progress_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    is_root: bool,
    inst: &BeesInstanceSnapshot,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Scan Progress (BEESSTATUS) ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &inst.status_file {
        FileAccess::Denied { path, .. } => {
            let hint = if is_root {
                format!("Permission denied reading {path}")
            } else {
                format!(
                    "Cannot read {path} as unprivileged user (RuntimeDirectoryMode=0700).\nRun with: sudo bees-tui   (or `sudo ~/.cargo/bin/bees-tui`) for live scan progress, counters, threads, disk I/O, and journal logs."
                )
            };
            frame.render_widget(
                Paragraph::new(hint)
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
        FileAccess::Missing { path } => {
            frame.render_widget(
                Paragraph::new(format!(
                    "{path} does not exist. Start {} or check BEESSTATUS.",
                    inst.unit_name
                )),
                inner,
            );
        }
        FileAccess::Error { error, .. } => {
            frame.render_widget(
                Paragraph::new(error.clone()).style(Style::default().fg(Color::Red)),
                inner,
            );
        }
        FileAccess::Ok { data, .. } => {
            let Some(progress) = &data.progress else {
                frame.render_widget(Paragraph::new("No PROGRESS block reported yet."), inner);
                return;
            };
            if !progress.available {
                frame.render_widget(
                    Paragraph::new(
                        progress
                            .raw_note
                            .as_deref()
                            .unwrap_or("[No progress estimate available]"),
                    ),
                    inner,
                );
                return;
            }

            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(1)])
                .split(inner);

            let rows: Vec<Row<'_>> = progress
                .rows
                .iter()
                .map(|r| {
                    let pct = r.point_percent.unwrap_or(0.0);
                    let bar = format!("{} {:5.1}%", unicode_bar(pct, 10), pct);
                    Row::new([
                        Cell::from(r.extsz.clone()),
                        Cell::from(r.datasz.clone()),
                        Cell::from(bar).style(Style::default().fg(Color::Cyan)),
                        Cell::from(format!("{}..{}", r.gen_min, r.gen_max)),
                        Cell::from(r.cycle_start.clone()),
                        Cell::from(r.tm_left.clone()),
                        Cell::from(r.next_cycle_eta.clone()),
                    ])
                })
                .collect();

            let table = Table::new(
                rows,
                [
                    Constraint::Length(5),
                    Constraint::Length(7),
                    Constraint::Length(18),
                    Constraint::Length(15),
                    Constraint::Length(17),
                    Constraint::Length(8),
                    Constraint::Min(10),
                ],
            )
            .header(
                Row::new(["Tier", "Data", "Cycle", "Gen", "Started", "Left", "ETA"]).style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            );
            frame.render_widget(table, split[0]);

            let footer = format!(
                "total {}   gen_now {}   updated {}",
                progress.total_size.as_deref().unwrap_or("—"),
                progress.gen_now.as_deref().unwrap_or("—"),
                progress.updated_at.as_deref().unwrap_or("—"),
            );
            frame.render_widget(
                Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
                split[1],
            );
        }
    }
}

fn render_workers_preview(frame: &mut Frame<'_>, area: Rect, inst: &BeesInstanceSnapshot) {
    let title = match &inst.status_file {
        FileAccess::Ok { data, .. } => match &data.threads {
            Some(th) => format!(
                " Workers ({}/{} tasks, {} thr, load {:.1}/{:.0}) ",
                th.work_queue_count,
                th.task_count,
                th.worker_count,
                th.load_current,
                th.load_target
            ),
            None => " Workers ".to_string(),
        },
        _ => " Workers ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let FileAccess::Ok { data, .. } = &inst.status_file else {
        frame.render_widget(
            Paragraph::new("Worker queue unavailable without BEESSTATUS read access.")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };
    let Some(th) = &data.threads else {
        frame.render_widget(Paragraph::new("No active worker notes."), inner);
        return;
    };

    let lines: Vec<Line<'_>> = th
        .threads
        .iter()
        .take(inner.height as usize)
        .map(|t| {
            let age = t
                .age_seconds
                .map(|a| format!("[{a:.1}s] "))
                .unwrap_or_default();
            let note_style = if t.throttled {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(
                    format!("{:>7} ", t.tid),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{}: ", t.name), Style::default().fg(Color::Cyan)),
                Span::styled(format!("{age}{}", t.note), note_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_journal_preview(frame: &mut Frame<'_>, area: Rect, inst: &BeesInstanceSnapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Recent Journal ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &inst.journal {
        JournalAccess::Ok(entries) => {
            let take_n = inner.height as usize;
            let start = entries.len().saturating_sub(take_n);
            let lines: Vec<Line<'_>> = entries[start..].iter().map(format_journal_line).collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        JournalAccess::Denied(msg) => {
            frame.render_widget(
                Paragraph::new(msg.clone())
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
        JournalAccess::Empty => {
            frame.render_widget(Paragraph::new("No journal entries."), inner);
        }
        JournalAccess::Error(err) => {
            frame.render_widget(
                Paragraph::new(err.clone()).style(Style::default().fg(Color::Red)),
                inner,
            );
        }
    }
}

fn format_journal_line(entry: &crate::model::JournalEntry) -> Line<'static> {
    let ts = entry
        .timestamp
        .split('T')
        .nth(1)
        .and_then(|s| s.get(..8))
        .unwrap_or(&entry.timestamp)
        .to_string();
    let msg_style = match entry.severity {
        JournalSeverity::Error => Style::default().fg(Color::Red),
        JournalSeverity::Warning => Style::default().fg(Color::Yellow),
        JournalSeverity::Info => Style::default(),
    };
    Line::from(vec![
        Span::styled(format!("{ts} "), Style::default().fg(Color::DarkGray)),
        Span::styled(entry.message.clone(), msg_style),
    ])
}

fn render_counters(
    frame: &mut Frame<'_>,
    area: Rect,
    scroll_counters: &mut usize,
    filter_query: &str,
    inst: &BeesInstanceSnapshot,
) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(area);

    let filter = filter_query.to_ascii_lowercase();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if filter.is_empty() {
            " Event Counters (TOTAL & RATES) ".to_string()
        } else {
            format!(" Event Counters [filter: {filter}] ")
        });
    let inner = block.inner(split[0]);
    frame.render_widget(block, split[0]);

    if let FileAccess::Ok { data, .. } = &inst.status_file {
        let mut items: Vec<(String, u64, Option<f64>)> = data
            .total
            .iter()
            .filter(|(k, _)| filter.is_empty() || k.to_ascii_lowercase().contains(&filter))
            .map(|(k, v)| {
                let rate = data.rates.iter().find(|(rk, _)| rk == k).map(|(_, rv)| *rv);
                (k.clone(), *v, rate)
            })
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        let visible_h = inner.height.saturating_sub(1) as usize;
        let max_scroll = items.len().saturating_sub(visible_h);
        *scroll_counters = (*scroll_counters).min(max_scroll);

        let rows: Vec<Row<'_>> = items
            .iter()
            .skip(*scroll_counters)
            .take(visible_h.max(1))
            .map(|(k, v, r)| {
                let style = if k.starts_with("bug_") || k.starts_with("exception_") {
                    Style::default().fg(Color::Red)
                } else if k.starts_with("dedup_bytes") || k.starts_with("dedup_hit") {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                Row::new([
                    Cell::from(k.clone()).style(style),
                    Cell::from(v.to_string()),
                    Cell::from(
                        r.map(|rv| format!("{rv:.3}/s"))
                            .unwrap_or_else(|| "—".to_string()),
                    ),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ],
        )
        .header(
            Row::new(["Counter", "Total", "Rate /s"]).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(table, inner);
    } else {
        frame.render_widget(
            Paragraph::new("Run with sudo bees-tui to inspect live BEESSTATUS counters.")
                .style(Style::default().fg(Color::Yellow)),
            inner,
        );
    }

    let crawl_block = Block::default()
        .borders(Borders::ALL)
        .title(" Crawl State (beescrawl.dat) ");
    let crawl_inner = crawl_block.inner(split[1]);
    frame.render_widget(crawl_block, split[1]);

    match &inst.crawl_file {
        FileAccess::Ok { data, .. } => {
            let rows: Vec<Row<'_>> = data
                .iter()
                .map(|c| {
                    Row::new([
                        Cell::from(c.root.to_string()),
                        Cell::from(c.objectid.to_string()),
                        Cell::from(c.offset.to_string()),
                        Cell::from(format!("{}..{}", c.min_transid, c.max_transid)),
                        Cell::from(c.start_ts.clone()),
                    ])
                })
                .collect();
            let table = Table::new(
                rows,
                [
                    Constraint::Length(6),
                    Constraint::Length(12),
                    Constraint::Length(14),
                    Constraint::Length(18),
                    Constraint::Min(19),
                ],
            )
            .header(
                Row::new(["Root", "ObjectID", "Offset", "TransID", "Started"]).style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            );
            frame.render_widget(table, crawl_inner);
        }
        FileAccess::Denied { .. } => {
            frame.render_widget(
                Paragraph::new("beescrawl.dat is root-only inside .beeshome/.")
                    .style(Style::default().fg(Color::DarkGray)),
                crawl_inner,
            );
        }
        FileAccess::Missing { path } => {
            frame.render_widget(
                Paragraph::new(format!("Missing {path} (written every 15m)."))
                    .style(Style::default().fg(Color::DarkGray)),
                crawl_inner,
            );
        }
        FileAccess::Error { error, .. } => {
            frame.render_widget(
                Paragraph::new(error.clone()).style(Style::default().fg(Color::Red)),
                crawl_inner,
            );
        }
    }
}

fn render_threads(
    frame: &mut Frame<'_>,
    area: Rect,
    scroll_threads: &mut usize,
    filter_query: &str,
    inst: &BeesInstanceSnapshot,
) {
    let filter = filter_query.to_ascii_lowercase();
    let title = match &inst.status_file {
        FileAccess::Ok { data, .. } => match &data.threads {
            Some(th) => format!(
                " Worker Threads (queue {}/{}, {} workers, load cur {:.1} / tgt {:.0} / avg {:.2}) ",
                th.work_queue_count,
                th.task_count,
                th.worker_count,
                th.load_current,
                th.load_target,
                th.load_average
            ),
            None => " Worker Threads ".to_string(),
        },
        _ => " Worker Threads ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let FileAccess::Ok { data, .. } = &inst.status_file else {
        frame.render_widget(
            Paragraph::new("Run with sudo bees-tui to inspect live worker threads.")
                .style(Style::default().fg(Color::Yellow)),
            inner,
        );
        return;
    };
    let Some(th) = &data.threads else {
        frame.render_widget(Paragraph::new("No THREADS section in BEESSTATUS."), inner);
        return;
    };

    let filtered: Vec<_> = th
        .threads
        .iter()
        .filter(|t| {
            filter.is_empty()
                || t.name.to_ascii_lowercase().contains(&filter)
                || t.note.to_ascii_lowercase().contains(&filter)
                || t.tid.to_string().contains(&filter)
        })
        .collect();

    let visible_h = inner.height.saturating_sub(1) as usize;
    let max_scroll = filtered.len().saturating_sub(visible_h);
    *scroll_threads = (*scroll_threads).min(max_scroll);

    let rows: Vec<Row<'_>> = filtered
        .iter()
        .skip(*scroll_threads)
        .take(visible_h.max(1))
        .map(|t| {
            let age = t
                .age_seconds
                .map(|a| format!("{a:.1}s"))
                .unwrap_or_else(|| "—".to_string());
            let style = if t.throttled {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(t.tid.to_string()),
                Cell::from(t.name.clone()).style(Style::default().fg(Color::Cyan)),
                Cell::from(age),
                Cell::from(t.note.clone()).style(style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(["TID", "Task", "Age", "Note"]).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    );
    frame.render_widget(table, inner);
}

fn render_logs(
    frame: &mut Frame<'_>,
    area: Rect,
    scroll_logs: &mut usize,
    logs_follow: bool,
    filter_query: &str,
    inst: &BeesInstanceSnapshot,
) {
    let filter = filter_query.to_ascii_lowercase();
    let follow_label = if logs_follow {
        "follow:on"
    } else {
        "follow:off"
    };
    let title = if filter.is_empty() {
        format!(" Journal ({}) [{follow_label}] ", inst.unit_name)
    } else {
        format!(
            " Journal ({}) [filter: {filter}] [{follow_label}] ",
            inst.unit_name
        )
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &inst.journal {
        JournalAccess::Ok(entries) => {
            let filtered: Vec<_> = entries
                .iter()
                .filter(|e| filter.is_empty() || e.message.to_ascii_lowercase().contains(&filter))
                .collect();
            let visible_h = inner.height as usize;
            let max_scroll = filtered.len().saturating_sub(visible_h);
            if logs_follow {
                *scroll_logs = max_scroll;
            } else {
                *scroll_logs = (*scroll_logs).min(max_scroll);
            }
            let lines: Vec<Line<'_>> = filtered
                .iter()
                .skip(*scroll_logs)
                .take(visible_h.max(1))
                .map(|e| format_journal_line(e))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        JournalAccess::Denied(msg) => {
            frame.render_widget(
                Paragraph::new(msg.clone()).style(Style::default().fg(Color::Yellow)),
                inner,
            );
        }
        JournalAccess::Empty => {
            frame.render_widget(Paragraph::new("No journal entries."), inner);
        }
        JournalAccess::Error(err) => {
            frame.render_widget(
                Paragraph::new(err.clone()).style(Style::default().fg(Color::Red)),
                inner,
            );
        }
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    if app.filter_active {
        let line = Line::from(vec![
            Span::styled(
                " /",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.filter_query.clone()),
            Span::styled("█  [Enter/Esc] Done", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    if let Some((msg, is_err)) = &app.flash {
        let style = if *is_err {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        frame.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), style)), area);
        return;
    }

    let mut hints = String::from(" [1-4/Tab] View  [r] Refresh  [p/u] Pause/Resume");
    if app.tab != Tab::Overview {
        hints.push_str("  [↑/↓] Scroll  [/] Filter");
    }
    if app.tab == Tab::Logs {
        hints.push_str("  [f] Follow");
    }
    if app.snapshot.as_ref().is_some_and(|s| s.instances.len() > 1) {
        hints.push_str("  [[/]] Unit");
    }
    hints.push_str("  [q] Quit");

    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_confirm_modal(frame: &mut Frame<'_>, area: Rect, app: &AppState, signal_label: &str) {
    let w = area.width.min(62);
    let h = 5u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);

    let pid_text = app
        .active_instance()
        .and_then(|i| i.unit.as_ref()?.main_pid)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "?".to_string());

    frame.render_widget(Clear, popup);
    let box_widget = Paragraph::new(format!(
        "Send {signal_label} to PID {pid_text}?\n\n[y] Confirm   [n / Esc] Cancel"
    ))
    .wrap(Wrap { trim: false })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Confirm Signal ")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(box_widget, popup);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::model::{
        BeesConfig, BeesHashStats, BeesProgress, BeesProgressRow, BeesStatusData, BeesThreadNote,
        BeesThreadsInfo, BtrfsUsage, JournalEntry, LoadAvg, ProcSample, SystemdBeesUnit,
    };

    fn sample_snapshot(unlocked: bool) -> BeesMonitorSnapshot {
        BeesMonitorSnapshot {
            collected_at_epoch_ms: 1_700_000_000_000,
            collected_at_hms: "19:15:00".to_string(),
            is_root: unlocked,
            host_load: Some(LoadAvg {
                load1: 11.4,
                load5: 11.1,
                load15: 10.8,
                runnable: 12,
                total_entities: 6400,
            }),
            instances: vec![BeesInstanceSnapshot {
                uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                unit_name: "beesd@11111111-2222-3333-4444-555555555555.service".to_string(),
                config: Some(BeesConfig {
                    file_path: "/etc/bees/11111111-2222-3333-4444-555555555555.conf".to_string(),
                    uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                    db_size_bytes: Some(4_294_967_296),
                    options: "--strip-paths --loadavg-target 12".to_string(),
                    loadavg_target: Some(12.0),
                    work_dir: "/run/bees".to_string(),
                    mnt_dir: "/run/bees/mnt/11111111-2222-3333-4444-555555555555".to_string(),
                    bees_home: "/run/bees/mnt/11111111-2222-3333-4444-555555555555/.beeshome"
                        .to_string(),
                    bees_status: "/run/bees/11111111-2222-3333-4444-555555555555.status"
                        .to_string(),
                }),
                unit: Some(SystemdBeesUnit {
                    unit: "beesd@11111111-2222-3333-4444-555555555555.service".to_string(),
                    uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                    load_state: "loaded".to_string(),
                    active_state: "active".to_string(),
                    sub_state: "running".to_string(),
                    unit_file_state: "enabled".to_string(),
                    main_pid: Some(12345),
                    tasks_current: Some(24),
                    memory_current_bytes: Some(19_497_414_656),
                    cpu_usage_nsec: Some(506_226_692_495_000),
                    started_at_raw: None,
                    runtime_directory_mode: Some("0700".to_string()),
                    env: Vec::new(),
                }),
                proc_sample: Some(ProcSample {
                    pid: 12345,
                    state: "S".to_string(),
                    threads: 24,
                    vm_rss_bytes: Some(4_443_205_632),
                    cpu_ticks: 50_743_776,
                    start_time_ticks: Some(726_707_728),
                    io_read_bytes: Some(10_000_000_000),
                    io_write_bytes: Some(500_000_000),
                }),
                uptime_secs: Some(93_600),
                cpu_percent: Some(420.5),
                io_read_bps: Some(52_428_800.0),
                io_write_bps: Some(1_048_576.0),
                cpu_history: vec![390, 410, 420],
                load_history: vec![1110, 1130, 1140],
                btrfs: Some(BtrfsUsage {
                    mount_point: "/mnt/btrfs".to_string(),
                    device_path: Some("/dev/loop0".to_string()),
                    device_size_bytes: Some(805_306_368_000),
                    used_bytes: Some(573_682_270_208),
                    free_estimated_bytes: Some(100_674_420_736),
                    data_size_bytes: Some(509_929_848_832),
                    data_used_bytes: Some(409_255_428_096),
                    data_used_percent: Some(80.26),
                    metadata_size_bytes: Some(147_654_180_864),
                    metadata_used_bytes: Some(82_213_322_752),
                    metadata_used_percent: Some(55.68),
                }),
                status_file: if unlocked {
                    FileAccess::Ok {
                        path: "/run/bees/11111111-2222-3333-4444-555555555555.status".to_string(),
                        data: BeesStatusData {
                            total: vec![
                                ("block_bytes".to_string(), 312_458_870_784),
                                ("dedup_bytes".to_string(), 14_710_261_760),
                                ("dedup_hit".to_string(), 48_291),
                                ("dedup_miss".to_string(), 91_234),
                            ],
                            rates: vec![
                                ("block_bytes".to_string(), 68_157_440.0),
                                ("dedup_bytes".to_string(), 3_250_585.0),
                            ],
                            threads: Some(BeesThreadsInfo {
                                work_queue_count: 12,
                                task_count: 240,
                                worker_count: 16,
                                load_current: 11.4,
                                load_target: 12.0,
                                load_average: 11.2,
                                threads: vec![BeesThreadNote {
                                    tid: 12346,
                                    name: "crawl_250".to_string(),
                                    age_seconds: Some(1.4),
                                    note: "scanning /mnt/btrfs/build.iso".to_string(),
                                    throttled: false,
                                }],
                            }),
                            progress: Some(BeesProgress {
                                available: true,
                                rows: vec![BeesProgressRow {
                                    extsz: "max".to_string(),
                                    datasz: "180G".to_string(),
                                    point: Some(642_000),
                                    point_percent: Some(64.2),
                                    gen_min: "180100".to_string(),
                                    gen_max: "184920".to_string(),
                                    cycle_start: "2026-11-25 04:00".to_string(),
                                    tm_left: "3h 12m".to_string(),
                                    next_cycle_eta: "2026-11-25 20:40".to_string(),
                                }],
                                total_size: Some("475G".to_string()),
                                gen_now: Some("184920".to_string()),
                                updated_at: Some("2026-11-25 17:28".to_string()),
                                raw_note: None,
                            }),
                        },
                    }
                } else {
                    FileAccess::Denied {
                        path: "/run/bees/11111111-2222-3333-4444-555555555555.status".to_string(),
                        error: "Permission denied".to_string(),
                    }
                },
                stats_file: if unlocked {
                    FileAccess::Ok {
                        path: "beesstats.txt".to_string(),
                        data: BeesHashStats {
                            now: None,
                            uptime_seconds: Some(88_200),
                            version: Some("v0.11".to_string()),
                            kernel: None,
                            occupied_cells: Some(214_748_364),
                            total_cells: Some(268_435_456),
                            occupied_percent: Some(80.0),
                            compressed_cells: Some(120_000_000),
                            compressed_percent: Some(55.9),
                            uncompressed_cells: Some(90_000_000),
                            uncompressed_percent: Some(41.9),
                            unaligned_eof_cells: Some(3_748_364),
                            unaligned_eof_percent: Some(1.7),
                            toxic_cells: Some(1_000_000),
                            toxic_percent: Some(0.5),
                        },
                    }
                } else {
                    FileAccess::Denied {
                        path: "beesstats.txt".to_string(),
                        error: "Permission denied".to_string(),
                    }
                },
                crawl_file: FileAccess::Missing {
                    path: "beescrawl.dat".to_string(),
                },
                journal: if unlocked {
                    JournalAccess::Ok(vec![JournalEntry {
                        timestamp: "2026-11-25T17:27:10-0800".to_string(),
                        message: "dedup: src [1204..1268] -> dst [880..944]".to_string(),
                        severity: JournalSeverity::Info,
                    }])
                } else {
                    JournalAccess::Denied("requires sudo".to_string())
                },
            }],
        }
    }

    #[test]
    fn renders_all_tabs_with_test_backend() {
        let mut app = AppState::new(1000);
        app.update_snapshot(sample_snapshot(true));

        for tab in Tab::ALL {
            app.tab = tab;
            let backend = TestBackend::new(120, 34);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let rendered: String = buf.content().iter().map(|c| c.symbol()).collect();
            assert!(rendered.contains("bees-tui"), "missing header on {tab:?}");
        }

        // Verify unprivileged narrow render doesn't panic
        app.tab = Tab::Overview;
        app.update_snapshot(sample_snapshot(false));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("sudo bees-tui"));
    }
}
