#[derive(Debug, Clone, PartialEq)]
pub struct BeesConfig {
    pub file_path: String,
    pub uuid: String,
    pub db_size_bytes: Option<u64>,
    pub options: String,
    pub loadavg_target: Option<f64>,
    pub work_dir: String,
    pub mnt_dir: String,
    pub bees_home: String,
    pub bees_status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemdBeesUnit {
    pub unit: String,
    pub uuid: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub main_pid: Option<u32>,
    pub tasks_current: Option<u64>,
    pub memory_current_bytes: Option<u64>,
    pub cpu_usage_nsec: Option<u64>,
    pub started_at_raw: Option<String>,
    pub runtime_directory_mode: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcSample {
    pub pid: u32,
    pub state: String,
    pub threads: u64,
    pub vm_rss_bytes: Option<u64>,
    pub cpu_ticks: u64,
    pub start_time_ticks: Option<u64>,
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadAvg {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub runnable: u64,
    pub total_entities: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BtrfsUsage {
    pub mount_point: String,
    pub device_path: Option<String>,
    pub device_size_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub free_estimated_bytes: Option<u64>,
    pub data_size_bytes: Option<u64>,
    pub data_used_bytes: Option<u64>,
    pub data_used_percent: Option<f64>,
    pub metadata_size_bytes: Option<u64>,
    pub metadata_used_bytes: Option<u64>,
    pub metadata_used_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesProgressRow {
    pub extsz: String,
    pub datasz: String,
    pub point: Option<u32>,
    pub point_percent: Option<f64>,
    pub gen_min: String,
    pub gen_max: String,
    pub cycle_start: String,
    pub tm_left: String,
    pub next_cycle_eta: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesProgress {
    pub available: bool,
    pub rows: Vec<BeesProgressRow>,
    pub total_size: Option<String>,
    pub gen_now: Option<String>,
    pub updated_at: Option<String>,
    pub raw_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesThreadNote {
    pub tid: u32,
    pub name: String,
    pub age_seconds: Option<f64>,
    pub note: String,
    pub throttled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesThreadsInfo {
    pub work_queue_count: u64,
    pub task_count: u64,
    pub worker_count: u64,
    pub load_current: f64,
    pub load_target: f64,
    pub load_average: f64,
    pub threads: Vec<BeesThreadNote>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesStatusData {
    pub total: Vec<(String, u64)>,
    pub rates: Vec<(String, f64)>,
    pub threads: Option<BeesThreadsInfo>,
    pub progress: Option<BeesProgress>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesHashStats {
    pub now: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub version: Option<String>,
    pub kernel: Option<String>,
    pub occupied_cells: Option<u64>,
    pub total_cells: Option<u64>,
    pub occupied_percent: Option<f64>,
    pub compressed_cells: Option<u64>,
    pub compressed_percent: Option<f64>,
    pub uncompressed_cells: Option<u64>,
    pub uncompressed_percent: Option<f64>,
    pub unaligned_eof_cells: Option<u64>,
    pub unaligned_eof_percent: Option<f64>,
    pub toxic_cells: Option<u64>,
    pub toxic_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesCrawlRow {
    pub root: u64,
    pub objectid: u64,
    pub offset: u64,
    pub min_transid: u64,
    pub max_transid: u64,
    pub started: u64,
    pub start_ts: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileAccess<T> {
    Ok { path: String, data: T },
    Denied { path: String, error: String },
    Missing { path: String },
    Error { path: String, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub timestamp: String,
    pub message: String,
    pub severity: JournalSeverity,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JournalAccess {
    Ok(Vec<JournalEntry>),
    Denied(String),
    Empty,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesInstanceSnapshot {
    pub uuid: String,
    pub unit_name: String,
    pub config: Option<BeesConfig>,
    pub unit: Option<SystemdBeesUnit>,
    pub proc_sample: Option<ProcSample>,
    pub uptime_secs: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub io_read_bps: Option<f64>,
    pub io_write_bps: Option<f64>,
    pub cpu_history: Vec<u64>,
    pub load_history: Vec<u64>,
    pub btrfs: Option<BtrfsUsage>,
    pub status_file: FileAccess<BeesStatusData>,
    pub stats_file: FileAccess<BeesHashStats>,
    pub crawl_file: FileAccess<Vec<BeesCrawlRow>>,
    pub journal: JournalAccess,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeesMonitorSnapshot {
    pub collected_at_epoch_ms: u64,
    pub collected_at_hms: String,
    pub is_root: bool,
    pub host_load: Option<LoadAvg>,
    pub instances: Vec<BeesInstanceSnapshot>,
}
