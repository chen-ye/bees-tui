use crate::model::{
    BeesConfig, BeesCrawlRow, BeesHashStats, BeesProgress, BeesProgressRow, BeesStatusData,
    BeesThreadNote, BeesThreadsInfo, BtrfsUsage, JournalEntry, JournalSeverity, LoadAvg,
    ProcSample, SystemdBeesUnit,
};

pub fn is_valid_uuid(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (idx, b) in bytes.iter().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

pub fn short_uuid(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

pub fn format_bytes(bytes: Option<u64>) -> String {
    let Some(b) = bytes else {
        return "—".to_string();
    };
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut val = b as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx + 1 < units.len() {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{b} B")
    } else if val >= 100.0 {
        format!("{val:.0} {}", units[idx])
    } else if val >= 10.0 {
        format!("{val:.1} {}", units[idx])
    } else {
        format!("{val:.2} {}", units[idx])
    }
}

pub fn format_rate_bytes(bytes_per_sec: Option<f64>) -> String {
    let Some(rate) = bytes_per_sec else {
        return "—".to_string();
    };
    if !rate.is_finite() || rate < 0.0 {
        return "—".to_string();
    }
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut val = rate;
    let mut idx = 0usize;
    while val >= 1024.0 && idx + 1 < units.len() {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{val:.0} B/s")
    } else if val >= 100.0 {
        format!("{val:.0} {}/s", units[idx])
    } else if val >= 10.0 {
        format!("{val:.1} {}/s", units[idx])
    } else {
        format!("{val:.2} {}/s", units[idx])
    }
}

pub fn format_duration_secs(secs: Option<u64>) -> String {
    let Some(total) = secs else {
        return "—".to_string();
    };
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    let s = total % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

pub fn unicode_bar(percent: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

fn lookup_var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
    vars.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn expand_vars(raw: &str, vars: &[(String, String)]) -> String {
    let mut out = raw.to_string();
    for _ in 0..3 {
        let mut next = String::with_capacity(out.len());
        let bytes = out.as_bytes();
        let mut i = 0;
        let mut changed = false;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                    if let Some(end_rel) = out[i + 2..].find('}') {
                        let name = &out[i + 2..i + 2 + end_rel];
                        if !name.is_empty()
                            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        {
                            next.push_str(lookup_var(vars, name).unwrap_or(""));
                            i += 2 + end_rel + 1;
                            changed = true;
                            continue;
                        }
                    }
                } else {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    if end > start {
                        let name = &out[start..end];
                        next.push_str(lookup_var(vars, name).unwrap_or(""));
                        i = end;
                        changed = true;
                        continue;
                    }
                }
            }
            if let Some(ch) = out[i..].chars().next() {
                next.push(ch);
                i += ch.len_utf8();
            } else {
                break;
            }
        }
        out = next;
        if !changed {
            break;
        }
    }
    out
}

fn eval_simple_arith(expr: &str) -> Option<u64> {
    let tokens: Vec<u8> = expr.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut pos = 0usize;

    fn parse_expr(toks: &[u8], pos: &mut usize) -> Option<i128> {
        let mut left = parse_term(toks, pos)?;
        while *pos < toks.len() && matches!(toks[*pos], b'+' | b'-') {
            let op = toks[*pos];
            *pos += 1;
            let right = parse_term(toks, pos)?;
            left = if op == b'+' {
                left.checked_add(right)?
            } else {
                left.checked_sub(right)?
            };
        }
        Some(left)
    }

    fn parse_term(toks: &[u8], pos: &mut usize) -> Option<i128> {
        let mut left = parse_factor(toks, pos)?;
        while *pos < toks.len() && matches!(toks[*pos], b'*' | b'/') {
            let op = toks[*pos];
            *pos += 1;
            let right = parse_factor(toks, pos)?;
            if op == b'*' {
                left = left.checked_mul(right)?;
            } else {
                if right == 0 {
                    return None;
                }
                left = left.checked_div(right)?;
            }
        }
        Some(left)
    }

    fn parse_factor(toks: &[u8], pos: &mut usize) -> Option<i128> {
        if *pos < toks.len() && toks[*pos] == b'(' {
            *pos += 1;
            let inner = parse_expr(toks, pos)?;
            if *pos >= toks.len() || toks[*pos] != b')' {
                return None;
            }
            *pos += 1;
            return Some(inner);
        }
        let start = *pos;
        while *pos < toks.len() && toks[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        std::str::from_utf8(&toks[start..*pos])
            .ok()?
            .parse::<i128>()
            .ok()
    }

    let val = parse_expr(&tokens, &mut pos)?;
    if pos != tokens.len() || val < 0 {
        return None;
    }
    u64::try_from(val).ok()
}

fn parse_option_f64(options: &str, short_flag: &str, long_flag: &str) -> Option<f64> {
    let parts: Vec<&str> = options.split_whitespace().collect();
    for (idx, tok) in parts.iter().enumerate() {
        if (*tok == short_flag || *tok == long_flag)
            && let Some(next) = parts.get(idx + 1)
            && let Ok(v) = next.parse::<f64>()
            && v.is_finite()
        {
            return Some(v);
        }
        if let Some(rest) = tok
            .strip_prefix(long_flag)
            .and_then(|r| r.strip_prefix('='))
            && let Ok(v) = rest.parse::<f64>()
            && v.is_finite()
        {
            return Some(v);
        }
        if let Some(rest) = tok
            .strip_prefix(short_flag)
            .and_then(|r| r.strip_prefix('='))
            && let Ok(v) = rest.parse::<f64>()
            && v.is_finite()
        {
            return Some(v);
        }
    }
    None
}

pub fn parse_bees_config(
    content: &str,
    file_path: &str,
    inherited_env: &[(String, String)],
) -> Option<BeesConfig> {
    let mut vars: Vec<(String, String)> = vec![
        ("AL128K".to_string(), (128 * 1024).to_string()),
        ("AL16M".to_string(), (16 * 1024 * 1024).to_string()),
    ];
    vars.extend(inherited_env.iter().cloned());

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let mut val = v.trim();
        if !val.starts_with('"')
            && !val.starts_with('\'')
            && let Some(idx) = val.find(" #")
        {
            val = val[..idx].trim();
        }

        if let Some(inner) = val.strip_prefix("$((").and_then(|r| r.strip_suffix("))")) {
            let expanded = expand_vars(inner, &vars);
            if let Some(num) = eval_simple_arith(&expanded) {
                vars.push((key.to_string(), num.to_string()));
                continue;
            }
        }
        let unquoted = strip_quotes(val);
        let resolved = if val.starts_with('\'') {
            unquoted.to_string()
        } else {
            expand_vars(unquoted, &vars)
        };
        vars.push((key.to_string(), resolved));
    }

    let uuid = lookup_var(&vars, "UUID")?.trim().to_lowercase();
    if !is_valid_uuid(&uuid) {
        return None;
    }

    let db_size_bytes = lookup_var(&vars, "DB_SIZE")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .or(Some(8192 * 128 * 1024));
    let options = lookup_var(&vars, "OPTIONS")
        .unwrap_or("")
        .trim()
        .to_string();
    let loadavg_target = parse_option_f64(&options, "-g", "--loadavg-target");

    let work_dir = lookup_var(&vars, "WORK_DIR")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("/run/bees")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let work_dir = if work_dir.is_empty() {
        "/run/bees".to_string()
    } else {
        work_dir
    };

    let mnt_dir = lookup_var(&vars, "MNT_DIR")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("{work_dir}/mnt/{uuid}"));

    let bees_home = lookup_var(&vars, "BEESHOME")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("{mnt_dir}/.beeshome"));

    let bees_status = lookup_var(&vars, "BEESSTATUS")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| format!("{work_dir}/{uuid}.status"));

    Some(BeesConfig {
        file_path: file_path.to_string(),
        uuid,
        db_size_bytes,
        options,
        loadavg_target,
        work_dir,
        mnt_dir,
        bees_home,
        bees_status,
    })
}

pub fn parse_systemd_unit_list(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let unit = trimmed.split_whitespace().next().unwrap_or("");
        if let Some(rest) = unit.strip_prefix("beesd@")
            && let Some(uuid) = rest.strip_suffix(".service")
        {
            let lower = uuid.to_lowercase();
            if is_valid_uuid(&lower) && !out.iter().any(|(u, _)| u == unit) {
                out.push((unit.to_string(), lower));
            }
        }
    }
    out
}

fn parse_env_pairs(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        let Some((k, after_eq)) = rest.split_once('=') else {
            break;
        };
        let key = k.trim();
        let after_eq = after_eq.trim_start();
        let (val, remainder) = if let Some(quoted) = after_eq.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                (&quoted[..end], quoted[end + 1..].trim_start())
            } else {
                (quoted, "")
            }
        } else if let Some((tok, tail)) = after_eq.split_once(char::is_whitespace) {
            (tok, tail.trim_start())
        } else {
            (after_eq, "")
        };
        if !key.is_empty() {
            out.push((key.to_string(), val.to_string()));
        }
        rest = remainder;
    }
    out
}

pub fn parse_systemd_show(output: &str, fallback_unit: &str) -> Option<SystemdBeesUnit> {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for line in output.lines() {
        if let Some((k, v)) = line.split_once('=') {
            pairs.push((k.trim(), v.trim()));
        }
    }
    let get =
        |k: &str| -> Option<&str> { pairs.iter().rev().find(|(pk, _)| *pk == k).map(|(_, v)| *v) };

    let unit = get("Id")
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_unit)
        .to_string();
    let uuid = unit
        .strip_prefix("beesd@")
        .and_then(|s| s.strip_suffix(".service"))
        .map(|s| s.to_lowercase())?;

    let parse_opt_u64 = |k: &str| -> Option<u64> {
        let v = get(k)?;
        if v == "[not set]" || v == "18446744073709551615" || v.is_empty() {
            return None;
        }
        v.parse::<u64>().ok()
    };

    let main_pid = parse_opt_u64("MainPID")
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0);

    Some(SystemdBeesUnit {
        unit,
        uuid,
        load_state: get("LoadState").unwrap_or("unknown").to_string(),
        active_state: get("ActiveState").unwrap_or("unknown").to_string(),
        sub_state: get("SubState").unwrap_or("unknown").to_string(),
        unit_file_state: get("UnitFileState").unwrap_or("unknown").to_string(),
        main_pid,
        tasks_current: parse_opt_u64("TasksCurrent"),
        memory_current_bytes: parse_opt_u64("MemoryCurrent"),
        cpu_usage_nsec: parse_opt_u64("CPUUsageNSec"),
        started_at_raw: get("ExecMainStartTimestamp")
            .filter(|s| !s.is_empty() && *s != "n/a")
            .map(ToString::to_string),
        runtime_directory_mode: get("RuntimeDirectoryMode")
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        env: get("Environment").map(parse_env_pairs).unwrap_or_default(),
    })
}

pub fn parse_proc_sample(
    stat_text: &str,
    status_text: &str,
    io_text: Option<&str>,
) -> Option<ProcSample> {
    let open = stat_text.find('(')?;
    let close = stat_text.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid = stat_text[..open].trim().parse::<u32>().ok()?;
    let rest: Vec<&str> = stat_text[close + 1..].split_whitespace().collect();
    let state = (*rest.first()?).to_string();
    let utime = rest.get(11)?.parse::<u64>().ok()?;
    let stime = rest.get(12)?.parse::<u64>().ok()?;
    let threads = rest.get(17)?.parse::<u64>().ok()?;
    let start_time_ticks = rest.get(19).and_then(|s| s.parse::<u64>().ok());

    let mut vm_rss_bytes = None;
    for line in status_text.lines() {
        if let Some(after) = line.strip_prefix("VmRSS:")
            && let Some(num) = after
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
        {
            vm_rss_bytes = Some(num * 1024);
        }
    }

    let mut io_read_bytes = None;
    let mut io_write_bytes = None;
    if let Some(io) = io_text {
        for line in io.lines() {
            if let Some(v) = line
                .strip_prefix("read_bytes:")
                .and_then(|s| s.trim().parse::<u64>().ok())
            {
                io_read_bytes = Some(v);
            } else if let Some(v) = line
                .strip_prefix("write_bytes:")
                .and_then(|s| s.trim().parse::<u64>().ok())
            {
                io_write_bytes = Some(v);
            }
        }
    }

    Some(ProcSample {
        pid,
        state,
        threads,
        vm_rss_bytes,
        cpu_ticks: utime.saturating_add(stime),
        start_time_ticks,
        io_read_bytes,
        io_write_bytes,
    })
}

pub fn parse_loadavg(text: &str) -> Option<LoadAvg> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let load1 = parts[0].parse::<f64>().ok()?;
    let load5 = parts[1].parse::<f64>().ok()?;
    let load15 = parts[2].parse::<f64>().ok()?;
    let (r_str, t_str) = parts[3].split_once('/')?;
    let runnable = r_str.parse::<u64>().ok()?;
    let total_entities = t_str.parse::<u64>().ok()?;
    if !(load1.is_finite() && load5.is_finite() && load15.is_finite()) {
        return None;
    }
    Some(LoadAvg {
        load1,
        load5,
        load15,
        runnable,
        total_entities,
    })
}

pub fn parse_findmnt(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(src) = parts.next() else {
            continue;
        };
        let Some(target) = parts.next() else {
            continue;
        };
        let device = src.split('[').next().unwrap_or(src).trim().to_string();
        let mount = target
            .trim_start_matches(['├', '└', '─', ' '])
            .trim()
            .to_string();
        if !device.is_empty() && !mount.is_empty() {
            out.push((device, mount));
        }
    }
    out
}

fn extract_u64_after(text: &str, prefix: &str) -> Option<u64> {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(prefix)
            && let Some(tok) = rest.split_whitespace().next()
            && let Ok(val) = tok.parse::<u64>()
        {
            return Some(val);
        }
    }
    None
}

fn parse_btrfs_profile_line(line: &str, kind_prefix: &str) -> Option<(u64, u64, f64)> {
    let t = line.trim();
    if !t.starts_with(kind_prefix) {
        return None;
    }
    let size_idx = t.find("Size:")?;
    let size_str = t[size_idx + 5..].split(',').next()?.trim();
    let size = size_str.parse::<u64>().ok()?;

    let used_idx = t.find("Used:")?;
    let after_used = &t[used_idx + 5..];
    let used_str = after_used.split_whitespace().next()?.trim();
    let used = used_str.parse::<u64>().ok()?;

    let pct_start = after_used.find('(')? + 1;
    let pct_end = after_used[pct_start..].find('%')? + pct_start;
    let pct = after_used[pct_start..pct_end].trim().parse::<f64>().ok()?;
    Some((size, used, pct))
}

pub fn parse_btrfs_usage(
    output: &str,
    mount_point: &str,
    device_path: Option<String>,
) -> Option<BtrfsUsage> {
    let device_size_bytes = extract_u64_after(output, "Device size:");
    let used_bytes = extract_u64_after(output, "Used:");
    let free_estimated_bytes = extract_u64_after(output, "Free (estimated):");

    let mut data_size_bytes = None;
    let mut data_used_bytes = None;
    let mut data_used_percent = None;
    let mut metadata_size_bytes = None;
    let mut metadata_used_bytes = None;
    let mut metadata_used_percent = None;

    for line in output.lines() {
        if let Some((sz, used, pct)) = parse_btrfs_profile_line(line, "Data,") {
            data_size_bytes = Some(sz);
            data_used_bytes = Some(used);
            data_used_percent = Some(pct);
        } else if let Some((sz, used, pct)) = parse_btrfs_profile_line(line, "Metadata,") {
            metadata_size_bytes = Some(sz);
            metadata_used_bytes = Some(used);
            metadata_used_percent = Some(pct);
        }
    }

    if device_size_bytes.is_none() && used_bytes.is_none() && data_size_bytes.is_none() {
        return None;
    }

    Some(BtrfsUsage {
        mount_point: mount_point.to_string(),
        device_path,
        device_size_bytes,
        used_bytes,
        free_estimated_bytes,
        data_size_bytes,
        data_used_bytes,
        data_used_percent,
        metadata_size_bytes,
        metadata_used_bytes,
        metadata_used_percent,
    })
}

fn parse_threads_header(line: &str) -> Option<(u64, u64, u64, f64, f64, f64)> {
    let inner = line.strip_prefix("THREADS (")?.strip_suffix("):")?;
    let (left, load_part) = inner.split_once("load:")?;
    let left_tokens: Vec<&str> = left.split_whitespace().collect();
    // work queue Q of T tasks, W workers,
    let q_idx = left_tokens.iter().position(|t| *t == "queue")?;
    let queue_cnt = left_tokens.get(q_idx + 1)?.parse::<u64>().ok()?;
    let task_cnt = left_tokens.get(q_idx + 3)?.parse::<u64>().ok()?;
    let w_idx = left_tokens.iter().position(|t| t.starts_with("worker"))?;
    let worker_cnt = left_tokens
        .get(w_idx.checked_sub(1)?)?
        .parse::<u64>()
        .ok()?;

    let load_tokens: Vec<&str> = load_part.split_whitespace().collect();
    let cur = load_tokens.get(1)?.parse::<f64>().ok()?;
    let tgt = load_tokens.get(3)?.parse::<f64>().ok()?;
    let avg = load_tokens.get(5)?.parse::<f64>().ok()?;
    Some((queue_cnt, task_cnt, worker_cnt, cur, tgt, avg))
}

fn parse_thread_note_line(line: &str) -> Option<BeesThreadNote> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("tid ")?;
    let (tid_str, after_tid) = rest.split_once(':')?;
    let tid = tid_str.trim().parse::<u32>().ok()?;
    let (name_str, after_name) = after_tid.split_once(':')?;
    let name = name_str.trim().to_string();

    let after_name = after_name.trim();
    let (age_seconds, note) = if let Some(bracketed) = after_name.strip_prefix('[')
        && let Some((age_raw, tail)) = bracketed.split_once("s]")
        && let Ok(age) = age_raw.trim().parse::<f64>()
    {
        (Some(age), tail.trim().to_string())
    } else {
        (None, after_name.to_string())
    };

    let throttled = note.to_ascii_lowercase().contains("throttle delay");
    Some(BeesThreadNote {
        tid,
        name,
        age_seconds,
        note,
        throttled,
    })
}

fn is_yyyy_mm_dd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn is_hh_mm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[..2].iter().all(u8::is_ascii_digit)
        && b[3..5].iter().all(u8::is_ascii_digit)
}

pub fn parse_progress_lines(lines: &[&str]) -> Option<BeesProgress> {
    let non_empty: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if non_empty.is_empty() {
        return None;
    }
    let first = non_empty[0].trim();
    if first.contains("No progress estimate available") {
        return Some(BeesProgress {
            available: false,
            rows: Vec::new(),
            total_size: None,
            gen_now: None,
            updated_at: None,
            raw_note: Some(first.to_string()),
        });
    }

    let dash_pos = non_empty.iter().position(|l| l.trim().starts_with("---"));
    let body = if let Some(idx) = dash_pos {
        &non_empty[idx + 1..]
    } else {
        &non_empty[..]
    };

    let mut rows = Vec::new();
    let mut total_size = None;
    let mut gen_now = None;
    let mut updated_at = None;

    for raw in body {
        let t = raw.trim();
        if t.is_empty() || t.starts_with("extsz ") {
            continue;
        }
        let tokens: Vec<&str> = t.split_whitespace().collect();
        if tokens.first() == Some(&"total") {
            total_size = tokens.get(1).map(|s| (*s).to_string());
            if let Some(pos) = tokens.iter().position(|tok| *tok == "gen_now") {
                gen_now = tokens.get(pos + 1).map(|s| (*s).to_string());
            }
            if let Some(pos) = tokens.iter().position(|tok| *tok == "updated") {
                updated_at = Some(tokens[pos + 1..].join(" "));
            }
            continue;
        }

        let Some(extsz) = tokens.first().copied() else {
            continue;
        };
        if !matches!(extsz, "max" | "32M" | "8M" | "2M" | "512K" | "128K") || tokens.len() < 8 {
            continue;
        }
        let datasz = tokens[1].to_string();
        let point_tok = tokens[2];
        let point = if point_tok.len() == 6 && point_tok.bytes().all(|b| b.is_ascii_digit()) {
            point_tok.parse::<u32>().ok()
        } else {
            None
        };
        let point_percent = point.map(|p| ((p as f64 / 999_999.0) * 100.0).clamp(0.0, 100.0));
        let gen_min = tokens[3].to_string();
        let gen_max = tokens[4].to_string();

        // Parse cycle_start (either "-" or "YYYY-MM-DD HH:MM")
        let mut idx = 5usize;
        let cycle_start =
            if idx + 1 < tokens.len() && is_yyyy_mm_dd(tokens[idx]) && is_hh_mm(tokens[idx + 1]) {
                let s = format!("{} {}", tokens[idx], tokens[idx + 1]);
                idx += 2;
                s
            } else if idx < tokens.len() {
                let s = tokens[idx].to_string();
                idx += 1;
                s
            } else {
                "-".to_string()
            };

        // Parse next_cycle_eta from end (either "YYYY-MM-DD HH:MM" or single token "idle" / "-")
        let (tm_left_end, next_cycle_eta) = if tokens.len() >= idx + 2
            && is_yyyy_mm_dd(tokens[tokens.len() - 2])
            && is_hh_mm(tokens[tokens.len() - 1])
        {
            (
                tokens.len() - 2,
                format!("{} {}", tokens[tokens.len() - 2], tokens[tokens.len() - 1]),
            )
        } else if tokens.len() > idx {
            (tokens.len() - 1, tokens[tokens.len() - 1].to_string())
        } else {
            (idx, "-".to_string())
        };

        let tm_left = if tm_left_end > idx {
            tokens[idx..tm_left_end].join(" ")
        } else {
            "-".to_string()
        };

        rows.push(BeesProgressRow {
            extsz: extsz.to_string(),
            datasz,
            point,
            point_percent,
            gen_min,
            gen_max,
            cycle_start,
            tm_left,
            next_cycle_eta,
        });
    }

    Some(BeesProgress {
        available: !rows.is_empty(),
        rows,
        total_size,
        gen_now,
        updated_at,
        raw_note: if non_empty.is_empty() {
            None
        } else {
            Some(non_empty.join("\n"))
        },
    })
}

pub fn parse_bees_status(text: &str) -> BeesStatusData {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        None,
        Total,
        Rates,
        Threads,
        Progress,
    }

    let mut section = Section::None;
    let mut total = Vec::new();
    let mut rates = Vec::new();
    let mut threads_header: Option<&str> = None;
    let mut thread_notes = Vec::new();
    let mut progress_lines = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "TOTAL:" {
            section = Section::Total;
            continue;
        }
        if trimmed == "RATES:" {
            section = Section::Rates;
            continue;
        }
        if trimmed.starts_with("THREADS (") {
            section = Section::Threads;
            threads_header = Some(trimmed);
            continue;
        }
        if trimmed == "PROGRESS:" {
            section = Section::Progress;
            continue;
        }

        match section {
            Section::None => {}
            Section::Total => {
                for tok in trimmed.split_whitespace() {
                    if let Some((k, v)) = tok.split_once('=')
                        && let Ok(n) = v.parse::<u64>()
                    {
                        total.push((k.to_string(), n));
                    }
                }
            }
            Section::Rates => {
                for tok in trimmed.split_whitespace() {
                    if let Some((k, v)) = tok.split_once('=')
                        && let Ok(n) = v.parse::<f64>()
                        && n.is_finite()
                    {
                        rates.push((k.to_string(), n));
                    }
                }
            }
            Section::Threads => {
                if let Some(note) = parse_thread_note_line(line) {
                    thread_notes.push(note);
                }
            }
            Section::Progress => progress_lines.push(line),
        }
    }

    let threads = threads_header.and_then(parse_threads_header).map(
        |(work_queue_count, task_count, worker_count, load_current, load_target, load_average)| {
            BeesThreadsInfo {
                work_queue_count,
                task_count,
                worker_count,
                load_current,
                load_target,
                load_average,
                threads: thread_notes,
            }
        },
    );

    BeesStatusData {
        total,
        rates,
        threads,
        progress: parse_progress_lines(&progress_lines),
    }
}

fn parse_cell_stat(text: &str, label: &str) -> Option<(u64, f64)> {
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for (idx, tok) in tokens.iter().enumerate() {
            if *tok == label
                && let Some(cnt_str) = tokens.get(idx + 1)
                && let Ok(cnt) = cnt_str.parse::<u64>()
                && let Some(pct_tok) = tokens.get(idx + 2)
                && let Some(pct_inner) =
                    pct_tok.strip_prefix('(').and_then(|s| s.strip_suffix("%)"))
                && let Ok(pct) = pct_inner.parse::<f64>()
            {
                return Some((cnt, pct));
            }
        }
    }
    None
}

pub fn parse_bees_stats(text: &str) -> BeesHashStats {
    let line_val = |prefix: &str| -> Option<String> {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(prefix).map(|v| v.trim().to_string()))
    };

    let uptime_seconds = line_val("Uptime:").and_then(|s| {
        s.split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok())
    });

    let mut occupied_cells = None;
    let mut total_cells = None;
    let mut occupied_percent = None;
    for line in text.lines() {
        if let Some(after) = line
            .trim()
            .strip_prefix("Hash table page occupancy histogram (")
            && let Some(inner) = after.strip_suffix(')')
            && let Some((frac, rest)) = inner.split_once(' ')
            && let Some((occ_s, tot_s)) = frac.split_once('/')
            && let (Ok(occ), Ok(tot)) = (occ_s.parse::<u64>(), tot_s.parse::<u64>())
        {
            occupied_cells = Some(occ);
            total_cells = Some(tot);
            if let Some(pct_tok) = rest.split(',').nth(1)
                && let Some(pct_s) = pct_tok.trim().strip_suffix('%')
                && let Ok(pct) = pct_s.parse::<f64>()
            {
                occupied_percent = Some(pct);
            }
        }
    }

    let compressed = parse_cell_stat(text, "compressed");
    let uncompressed = parse_cell_stat(text, "uncompressed");
    let unaligned_eof = parse_cell_stat(text, "unaligned_eof");
    let toxic = parse_cell_stat(text, "toxic");

    BeesHashStats {
        now: line_val("Now:"),
        uptime_seconds,
        version: line_val("Version:"),
        kernel: line_val("Kernel:"),
        occupied_cells,
        total_cells,
        occupied_percent,
        compressed_cells: compressed.map(|(c, _)| c),
        compressed_percent: compressed.map(|(_, p)| p),
        uncompressed_cells: uncompressed.map(|(c, _)| c),
        uncompressed_percent: uncompressed.map(|(_, p)| p),
        unaligned_eof_cells: unaligned_eof.map(|(c, _)| c),
        unaligned_eof_percent: unaligned_eof.map(|(_, p)| p),
        toxic_cells: toxic.map(|(c, _)| c),
        toxic_percent: toxic.map(|(_, p)| p),
    }
}

pub fn parse_bees_crawl(text: &str) -> Vec<BeesCrawlRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 14 {
            continue;
        }
        let get_u64 = |key: &str| -> Option<u64> {
            let pos = tokens.iter().position(|t| *t == key)?;
            tokens.get(pos + 1)?.parse::<u64>().ok()
        };
        let get_str = |key: &str| -> Option<String> {
            let pos = tokens.iter().position(|t| *t == key)?;
            tokens.get(pos + 1).map(|s| (*s).to_string())
        };
        if let (
            Some(root),
            Some(objectid),
            Some(offset),
            Some(min_transid),
            Some(max_transid),
            Some(started),
            Some(start_ts),
        ) = (
            get_u64("root"),
            get_u64("objectid"),
            get_u64("offset"),
            get_u64("min_transid"),
            get_u64("max_transid"),
            get_u64("started"),
            get_str("start_ts"),
        ) {
            rows.push(BeesCrawlRow {
                root,
                objectid,
                offset,
                min_transid,
                max_transid,
                started,
                start_ts,
            });
        }
    }
    rows
}

pub fn parse_journal_lines(output: &str) -> Vec<JournalEntry> {
    let mut out = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("-- ") {
            continue;
        }
        let mut parts = t.splitn(4, char::is_whitespace);
        let (timestamp, message) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(ts), Some(_host), Some(_unit), Some(msg)) => {
                (ts.to_string(), msg.trim().to_string())
            }
            _ => (String::new(), t.to_string()),
        };
        let lower = message.to_ascii_lowercase();
        let severity = if lower.contains("error")
            || lower.contains("bug_")
            || lower.contains("exception")
            || lower.contains("failed")
        {
            JournalSeverity::Error
        } else if lower.contains("warn")
            || lower.contains("throttle")
            || lower.contains("performance:")
        {
            JournalSeverity::Warning
        } else {
            JournalSeverity::Info
        };
        out.push(JournalEntry {
            timestamp,
            message,
            severity,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bees_config_and_arithmetic() {
        let cfg = parse_bees_config(
            r#"
UUID="11111111-2222-3333-4444-555555555555"
DB_SIZE=$((1024*1024*1024))
OPTIONS="--strip-paths --loadavg-target 12"
"#,
            "/etc/bees/test.conf",
            &[],
        )
        .expect("valid config");
        assert_eq!(cfg.uuid, "11111111-2222-3333-4444-555555555555");
        assert_eq!(cfg.db_size_bytes, Some(1_073_741_824));
        assert_eq!(cfg.loadavg_target, Some(12.0));
        assert_eq!(
            cfg.bees_status,
            "/run/bees/11111111-2222-3333-4444-555555555555.status"
        );
    }

    #[test]
    fn parses_systemd_show_and_unit_list() {
        let list = parse_systemd_unit_list(
            "beesd@11111111-2222-3333-4444-555555555555.service loaded active running Bees\n",
        );
        assert_eq!(list.len(), 1);
        let unit = parse_systemd_show(
            "Id=beesd@11111111-2222-3333-4444-555555555555.service\nActiveState=active\nSubState=running\nMainPID=12345\nTasksCurrent=32\nMemoryCurrent=19497414656\nCPUUsageNSec=506226692495000\nRuntimeDirectoryMode=0700\n",
            "beesd@11111111-2222-3333-4444-555555555555.service",
        )
        .expect("unit");
        assert_eq!(unit.main_pid, Some(12_345));
        assert_eq!(unit.tasks_current, Some(32));
        assert_eq!(unit.runtime_directory_mode.as_deref(), Some("0700"));
    }

    #[test]
    fn parses_proc_loadavg_findmnt_btrfs() {
        let stat = "12345 (bees) S 1 12345 12345 0 -1 4194560 5794457 3487 34723 27 4495437 46131547 0 0 39 19 33 0 726707728 7199453184 1084726 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 6 0 3 0 0 0 0 0 0 0 0 0 0 0";
        let status = "VmRSS:\t 4338904 kB\n";
        let io = "read_bytes: 123456\nwrite_bytes: 654321\n";
        let proc_sample = parse_proc_sample(stat, status, Some(io)).expect("proc");
        assert_eq!(proc_sample.threads, 33);
        assert_eq!(proc_sample.vm_rss_bytes, Some(4_338_904 * 1024));
        assert_eq!(proc_sample.io_read_bytes, Some(123_456));
        assert_eq!(proc_sample.io_write_bytes, Some(654_321));

        let load = parse_loadavg("11.81 11.28 11.55 11/6729 1897610\n").expect("load");
        assert_eq!(load.load1, 11.81);
        assert_eq!(load.runnable, 11);

        let mounts = parse_findmnt("/dev/loop0[/@data]   /mnt/btrfs\n");
        assert_eq!(
            mounts,
            vec![("/dev/loop0".to_string(), "/mnt/btrfs".to_string())]
        );

        let btrfs = parse_btrfs_usage(
            "Overall:\n    Device size: 805306368000\n    Used: 572608172032\n    Free (estimated): 100881444864\nData,single: Size:509929848832, Used:409048403968 (80.22%)\nMetadata,DUP: Size:147654180864, Used:81779785728 (55.39%)\n",
            "/mnt/btrfs",
            Some("/dev/loop0".to_string()),
        )
        .expect("btrfs");
        assert_eq!(btrfs.data_used_percent, Some(80.22));
        assert_eq!(btrfs.metadata_used_percent, Some(55.39));
    }

    #[test]
    fn parses_bees_status_stats_and_crawl() {
        let status_txt = r#"TOTAL:
	block_bytes=1073741824 dedup_bytes=268435456
	dedup_hit=1024 dedup_miss=4096
RATES:
	block_bytes=10485760.5 dedup_bytes=2097152.25
THREADS (work queue 14 of 256 tasks, 12 workers, load: current 11.4 target 12 average 11.8):
	tid 1001: crawl_250: [6.2s] scanning /mnt/btrfs/file.bin
	tid 1002: dedup_1: dedup: throttle delay 1.5 s, time used 0.5 s, avg time 1.2 s
PROGRESS:
extsz  datasz  point   gen_min  gen_max  this cycle start  tm_left  next cycle ETA
-----  ------  ------  -------  -------  ----------------  -------  --------------
  max   120G   250000     1000     1500  2026-11-25 12:00   1h 23m  2026-11-25 13:23
 128K   100G   999999     1500     1500  2026-11-25 10:00   -       idle
total  220G           gen_now  1500                        updated  2026-11-25 12:05
"#;
        let parsed = parse_bees_status(status_txt);
        assert_eq!(parsed.total.len(), 4);
        assert_eq!(parsed.rates.len(), 2);
        let th = parsed.threads.expect("threads");
        assert_eq!(th.work_queue_count, 14);
        assert_eq!(th.threads.len(), 2);
        assert!(th.threads[1].throttled);
        let prog = parsed.progress.expect("progress");
        assert!(prog.available);
        assert_eq!(prog.rows.len(), 2);
        assert_eq!(prog.rows[0].tm_left, "1h 23m");
        assert_eq!(prog.rows[1].next_cycle_eta, "idle");
        assert_eq!(prog.gen_now.as_deref(), Some("1500"));

        let stats_txt = "Now: 2026-11-25-12-00-00\nUptime: 86400 seconds\nVersion: v0.11\nKernel: Linux 6.12\nHash table page occupancy histogram (210/268 cells occupied, 78.36%)\ncompressed 120 (57.1%)\nuncompressed 85 (40.5%) unaligned_eof 4 (1.9%) toxic 1 (0.5%)\n";
        let stats = parse_bees_stats(stats_txt);
        assert_eq!(stats.occupied_percent, Some(78.36));
        assert_eq!(stats.toxic_cells, Some(1));

        let crawl = parse_bees_crawl(
            "root 250 objectid 256 offset 4096 min_transid 100 max_transid 200 started 1700000000 start_ts 2026-11-25-10-00-00\n",
        );
        assert_eq!(crawl.len(), 1);
        assert_eq!(crawl[0].root, 250);
    }
}
