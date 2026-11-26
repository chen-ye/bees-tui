use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::{
    BeesConfig, BeesCrawlRow, BeesHashStats, BeesInstanceSnapshot, BeesMonitorSnapshot,
    BeesStatusData, FileAccess, JournalAccess, ProcSample, SystemdBeesUnit,
};
use crate::parsers::{
    parse_bees_config, parse_bees_crawl, parse_bees_stats, parse_bees_status, parse_btrfs_usage,
    parse_findmnt, parse_journal_lines, parse_loadavg, parse_proc_sample, parse_systemd_show,
    parse_systemd_unit_list,
};

const HISTORY_MAX: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeesSignal {
    Pause,
    Resume,
}

impl BeesSignal {
    pub fn as_kill_flag(self) -> &'static str {
        match self {
            Self::Pause => "-USR1",
            Self::Resume => "-USR2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pause => "SIGUSR1 (pause workers)",
            Self::Resume => "SIGUSR2 (resume workers)",
        }
    }
}

pub fn send_bees_signal(pid: u32, sig: BeesSignal) -> Result<(), String> {
    let out = Command::new("kill")
        .arg(sig.as_kill_flag())
        .arg(pid.to_string())
        .output()
        .map_err(|e| format!("failed to run kill: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("kill exited with {}", out.status)
        } else {
            err
        })
    }
}

pub fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
}

#[derive(Debug, Clone)]
struct PrevSample {
    pid: u32,
    cpu_ticks: u64,
    io_read_bytes: Option<u64>,
    io_write_bytes: Option<u64>,
    at: Instant,
}

pub struct BeesCollector {
    config_dir: PathBuf,
    proc_dir: PathBuf,
    unit_filter: Option<String>,
    clk_tck: u64,
    is_root: bool,
    prev_by_uuid: HashMap<String, PrevSample>,
    cpu_history: HashMap<String, VecDeque<u64>>,
    load_history: HashMap<String, VecDeque<u64>>,
}

impl BeesCollector {
    pub fn new(config_dir: PathBuf, unit_filter: Option<String>) -> Self {
        Self {
            config_dir,
            proc_dir: PathBuf::from("/proc"),
            unit_filter: unit_filter.map(|s| s.to_lowercase()),
            clk_tck: detect_clk_tck(),
            is_root: detect_euid_zero(),
            prev_by_uuid: HashMap::new(),
            cpu_history: HashMap::new(),
            load_history: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn with_paths(config_dir: PathBuf, proc_dir: PathBuf) -> Self {
        Self {
            config_dir,
            proc_dir,
            unit_filter: None,
            clk_tck: 100,
            is_root: false,
            prev_by_uuid: HashMap::new(),
            cpu_history: HashMap::new(),
            load_history: HashMap::new(),
        }
    }

    pub fn collect(&mut self) -> BeesMonitorSnapshot {
        let now_instant = Instant::now();
        let now_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let collected_at_hms = current_hms();

        let configs = read_config_files(&self.config_dir);
        let active_units = run_cmd(
            "systemctl",
            &[
                "list-units",
                "beesd@*",
                "--all",
                "--no-legend",
                "--no-pager",
                "--plain",
            ],
        );
        let unit_files = run_cmd(
            "systemctl",
            &["list-unit-files", "beesd@*", "--no-legend", "--no-pager"],
        );

        let mut unit_names: HashMap<String, String> = HashMap::new();
        for (unit, uuid) in parse_systemd_unit_list(&active_units.stdout) {
            unit_names.insert(uuid, unit);
        }
        for (unit, uuid) in parse_systemd_unit_list(&unit_files.stdout) {
            unit_names.entry(uuid).or_insert(unit);
        }

        let mut uuids: BTreeSet<String> = configs.keys().cloned().collect();
        uuids.extend(unit_names.keys().cloned());

        if let Some(filter) = &self.unit_filter {
            uuids.retain(|u| {
                let unit = unit_names.get(u).map(String::as_str).unwrap_or("");
                u == filter
                    || u.starts_with(filter)
                    || unit.to_lowercase() == *filter
                    || unit.to_lowercase().contains(filter)
            });
        }

        let host_load = fs::read_to_string(self.proc_dir.join("loadavg"))
            .ok()
            .and_then(|s| parse_loadavg(&s));
        let host_uptime_secs = fs::read_to_string(self.proc_dir.join("uptime"))
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok());
        let findmnt_res = run_cmd("findmnt", &["-t", "btrfs", "-no", "SOURCE,TARGET"]);
        let btrfs_mounts = if findmnt_res.ok {
            parse_findmnt(&findmnt_res.stdout)
        } else {
            Vec::new()
        };

        let mut instances = Vec::with_capacity(uuids.len());
        for uuid in uuids {
            let unit_name = unit_names
                .get(&uuid)
                .cloned()
                .unwrap_or_else(|| format!("beesd@{uuid}.service"));
            let unit = read_systemd_unit(&unit_name);
            let env_slice: Vec<(String, String)> =
                unit.as_ref().map(|u| u.env.clone()).unwrap_or_default();
            let config: Option<BeesConfig> = configs
                .get(&uuid)
                .and_then(|(path, content)| parse_bees_config(content, path, &env_slice));

            let status_path = config
                .as_ref()
                .map(|c| c.bees_status.clone())
                .unwrap_or_else(|| format!("/run/bees/{uuid}.status"));
            let bees_home = config
                .as_ref()
                .map(|c| c.bees_home.clone())
                .unwrap_or_else(|| format!("/run/bees/mnt/{uuid}/.beeshome"));

            let (device_path, mount_point) = resolve_mount_point(&uuid, &btrfs_mounts);
            let btrfs = mount_point.as_deref().and_then(|mp| {
                let res = run_cmd("btrfs", &["filesystem", "usage", "-b", mp]);
                parse_btrfs_usage(&res.stdout, mp, device_path.clone())
            });

            let proc_sample = unit
                .as_ref()
                .and_then(|u| u.main_pid)
                .and_then(|pid| read_proc_for_pid(&self.proc_dir, pid));

            let uptime_secs = proc_sample
                .as_ref()
                .and_then(|p| p.start_time_ticks)
                .zip(host_uptime_secs)
                .map(|(start_ticks, host_up)| {
                    let proc_start = start_ticks as f64 / self.clk_tck.max(1) as f64;
                    (host_up - proc_start).max(0.0).round() as u64
                });

            let (cpu_percent, io_read_bps, io_write_bps) =
                self.compute_rates(&uuid, proc_sample.as_ref(), now_instant);

            let cpu_hist = if let Some(cpu) = cpu_percent {
                push_history(&mut self.cpu_history, &uuid, cpu.round().max(0.0) as u64)
            } else {
                self.cpu_history
                    .get(&uuid)
                    .map(|q| q.iter().copied().collect())
                    .unwrap_or_default()
            };

            let load_hist = if let Some(load) = &host_load {
                push_history(
                    &mut self.load_history,
                    &uuid,
                    (load.load1 * 100.0).round().max(0.0) as u64,
                )
            } else {
                self.load_history
                    .get(&uuid)
                    .map(|q| q.iter().copied().collect())
                    .unwrap_or_default()
            };

            let status_file: FileAccess<BeesStatusData> =
                read_parsed_file(&status_path, parse_bees_status);

            let mut stats_file: FileAccess<BeesHashStats> = read_parsed_file(
                &Path::new(&bees_home)
                    .join("beesstats.txt")
                    .to_string_lossy(),
                parse_bees_stats,
            );
            if !matches!(stats_file, FileAccess::Ok { .. })
                && let Some(mp) = &mount_point
            {
                let fallback = Path::new(mp).join(".beeshome").join("beesstats.txt");
                let alt = read_parsed_file(&fallback.to_string_lossy(), parse_bees_stats);
                if matches!(alt, FileAccess::Ok { .. }) {
                    stats_file = alt;
                }
            }

            let mut crawl_file: FileAccess<Vec<BeesCrawlRow>> = read_parsed_file(
                &Path::new(&bees_home)
                    .join("beescrawl.dat")
                    .to_string_lossy(),
                parse_bees_crawl,
            );
            if !matches!(crawl_file, FileAccess::Ok { .. })
                && let Some(mp) = &mount_point
            {
                let fallback = Path::new(mp).join(".beeshome").join("beescrawl.dat");
                let alt = read_parsed_file(&fallback.to_string_lossy(), parse_bees_crawl);
                if matches!(alt, FileAccess::Ok { .. }) {
                    crawl_file = alt;
                }
            }

            let journal = read_journal(&unit_name);

            instances.push(BeesInstanceSnapshot {
                uuid,
                unit_name,
                config,
                unit,
                proc_sample,
                uptime_secs,
                cpu_percent,
                io_read_bps,
                io_write_bps,
                cpu_history: cpu_hist,
                load_history: load_hist,
                btrfs,
                status_file,
                stats_file,
                crawl_file,
                journal,
            });
        }

        BeesMonitorSnapshot {
            collected_at_epoch_ms: now_epoch_ms,
            collected_at_hms,
            is_root: self.is_root,
            host_load,
            instances,
        }
    }

    fn compute_rates(
        &mut self,
        uuid: &str,
        proc_sample: Option<&ProcSample>,
        now: Instant,
    ) -> (Option<f64>, Option<f64>, Option<f64>) {
        let Some(p) = proc_sample else {
            self.prev_by_uuid.remove(uuid);
            return (None, None, None);
        };

        let mut cpu_pct = None;
        let mut read_bps = None;
        let mut write_bps = None;

        if let Some(prev) = self.prev_by_uuid.get(uuid)
            && prev.pid == p.pid
        {
            let dt = now.saturating_duration_since(prev.at).as_secs_f64();
            if dt >= 0.05 {
                let d_ticks = p.cpu_ticks.saturating_sub(prev.cpu_ticks);
                cpu_pct = Some(((d_ticks as f64 / self.clk_tck.max(1) as f64) / dt) * 100.0);

                if let (Some(curr_r), Some(prev_r)) = (p.io_read_bytes, prev.io_read_bytes) {
                    read_bps = Some(curr_r.saturating_sub(prev_r) as f64 / dt);
                }
                if let (Some(curr_w), Some(prev_w)) = (p.io_write_bytes, prev.io_write_bytes) {
                    write_bps = Some(curr_w.saturating_sub(prev_w) as f64 / dt);
                }
            }
        }

        self.prev_by_uuid.insert(
            uuid.to_string(),
            PrevSample {
                pid: p.pid,
                cpu_ticks: p.cpu_ticks,
                io_read_bytes: p.io_read_bytes,
                io_write_bytes: p.io_write_bytes,
                at: now,
            },
        );
        (cpu_pct, read_bps, write_bps)
    }
}

fn push_history(map: &mut HashMap<String, VecDeque<u64>>, key: &str, val: u64) -> Vec<u64> {
    let entry = map.entry(key.to_string()).or_default();
    if entry.len() >= HISTORY_MAX {
        entry.pop_front();
    }
    entry.push_back(val);
    entry.iter().copied().collect()
}

struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run_cmd(bin: &str, args: &[&str]) -> CmdOut {
    match Command::new(bin).args(args).output() {
        Ok(out) => CmdOut {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => CmdOut {
            ok: false,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

fn detect_clk_tck() -> u64 {
    let out = run_cmd("getconf", &["CLK_TCK"]);
    if out.ok
        && let Ok(v) = out.stdout.trim().parse::<u64>()
        && v > 0
    {
        return v;
    }
    100
}

fn detect_euid_zero() -> bool {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return false;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut cols = rest.split_whitespace();
            let _real = cols.next();
            if let Some(effective) = cols.next() {
                return effective == "0";
            }
        }
    }
    false
}

fn current_hms() -> String {
    let out = run_cmd("date", &["+%H:%M:%S"]);
    if out.ok {
        let s = out.stdout.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn read_config_files(config_dir: &Path) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    let Ok(entries) = fs::read_dir(config_dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("beesd.conf.sample") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path)
            && let Some(cfg) = parse_bees_config(&content, &path.to_string_lossy(), &[])
        {
            map.insert(cfg.uuid, (path.to_string_lossy().into_owned(), content));
        }
    }
    map
}

fn read_systemd_unit(unit_name: &str) -> Option<SystemdBeesUnit> {
    let props = "Id,LoadState,ActiveState,SubState,UnitFileState,MainPID,TasksCurrent,MemoryCurrent,CPUUsageNSec,ExecMainStartTimestamp,RuntimeDirectoryMode,Environment";
    let res = run_cmd(
        "systemctl",
        &[
            "show",
            unit_name,
            &format!("--property={props}"),
            "--no-pager",
        ],
    );
    if !res.ok && res.stdout.trim().is_empty() {
        return None;
    }
    parse_systemd_show(&res.stdout, unit_name)
}

fn read_proc_for_pid(proc_dir: &Path, pid: u32) -> Option<ProcSample> {
    let pdir = proc_dir.join(pid.to_string());
    let stat = fs::read_to_string(pdir.join("stat")).ok()?;
    let status = fs::read_to_string(pdir.join("status")).unwrap_or_default();
    let io = fs::read_to_string(pdir.join("io")).ok();
    parse_proc_sample(&stat, &status, io.as_deref())
}

fn resolve_mount_point(
    uuid: &str,
    mounts: &[(String, String)],
) -> (Option<String>, Option<String>) {
    let Ok(device_real) = fs::canonicalize(format!("/dev/disk/by-uuid/{uuid}")) else {
        return (None, None);
    };
    let device_str = device_real.to_string_lossy().into_owned();
    let mut matches: Vec<String> = mounts
        .iter()
        .filter(|(dev, _)| {
            fs::canonicalize(dev)
                .map(|p| p == device_real)
                .unwrap_or_else(|_| dev == &device_str)
        })
        .map(|(_, mp)| mp.clone())
        .collect();
    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    (Some(device_str), matches.into_iter().next())
}

fn read_parsed_file<T>(path: &str, parse: impl FnOnce(&str) -> T) -> FileAccess<T> {
    match fs::read_to_string(path) {
        Ok(text) => FileAccess::Ok {
            path: path.to_string(),
            data: parse(&text),
        },
        Err(e) => match e.kind() {
            ErrorKind::NotFound => FileAccess::Missing {
                path: path.to_string(),
            },
            ErrorKind::PermissionDenied => FileAccess::Denied {
                path: path.to_string(),
                error: e.to_string(),
            },
            _ => FileAccess::Error {
                path: path.to_string(),
                error: e.to_string(),
            },
        },
    }
}

fn read_journal(unit_name: &str) -> JournalAccess {
    let res = run_cmd(
        "journalctl",
        &[
            "-u",
            unit_name,
            "-n",
            "200",
            "--no-pager",
            "--output=short-iso",
        ],
    );
    let combined = format!("{}\n{}", res.stdout, res.stderr);
    if combined.contains("not seeing messages from other users")
        || combined.contains("insufficient permissions")
        || combined.contains("systemd-journal")
        || combined.contains("No data available")
    {
        return JournalAccess::Denied(
            "Journal access requires sudo or systemd-journal group.".to_string(),
        );
    }
    if !res.ok {
        let msg = res.stderr.trim();
        return JournalAccess::Error(if msg.is_empty() {
            "journalctl failed".to_string()
        } else {
            msg.to_string()
        });
    }
    let entries = parse_journal_lines(&res.stdout);
    if entries.is_empty() {
        JournalAccess::Empty
    } else {
        JournalAccess::Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_temp_config_and_proc_loadavg() {
        let base = std::env::temp_dir().join(format!(
            "bees-tui-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg_dir = base.join("etc-bees");
        let proc_dir = base.join("proc");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::create_dir_all(&proc_dir).unwrap();

        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let status_file = base.join("test.status");
        fs::write(
            cfg_dir.join(format!("{uuid}.conf")),
            format!("UUID=\"{uuid}\"\nDB_SIZE=1073741824\nOPTIONS=\"--loadavg-target 6\"\nBEESSTATUS=\"{}\"\n", status_file.display()),
        )
        .unwrap();
        fs::write(proc_dir.join("loadavg"), "2.10 1.90 1.75 3/250 9999\n").unwrap();
        fs::write(
            &status_file,
            "TOTAL:\n\tdedup_bytes=131072\nRATES:\n\tdedup_bytes=4096\nPROGRESS:\n[No progress estimate available]\n",
        )
        .unwrap();

        let mut collector = BeesCollector::with_paths(cfg_dir, proc_dir);
        let snap = collector.collect();
        assert_eq!(snap.host_load.as_ref().map(|l| l.load1), Some(2.10));
        let inst = snap
            .instances
            .iter()
            .find(|i| i.uuid == uuid)
            .expect("instance");
        match &inst.status_file {
            FileAccess::Ok { data, .. } => {
                assert_eq!(data.total, vec![("dedup_bytes".to_string(), 131_072)]);
            }
            other => panic!("expected Ok status file, got {other:?}"),
        }

        let _ = fs::remove_dir_all(base);
    }
}
