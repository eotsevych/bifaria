use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemVitals {
    pub cpu_brand: String,
    pub cpu_cores_logical: usize,
    pub cpu_usage_percent: f32,
    pub load_avg_one: f64,
    pub load_avg_five: f64,
    pub load_avg_fifteen: f64,
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub free_memory_mb: u64,
    pub available_memory_mb: u64,
    pub process_rss_mb: u64,
}

/// This process's resident memory in MB, refreshing only this process (cheap enough to call
/// around a model load).
pub fn process_rss_mb() -> u64 {
    let Ok(pid) = sysinfo::get_current_pid() else {
        return 0;
    };
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_memory());
    sys.process(pid).map(|p| p.memory() / (1024 * 1024)).unwrap_or(0)
}

/// Memory the system can still hand out, in MB, refreshing only memory.
pub fn available_memory_mb() -> u64 {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory() / (1024 * 1024)
}

impl SystemVitals {
    pub fn collect() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();

        let cpu_brand = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_else(|| "Unknown CPU".to_string());

        let cpu_cores_logical = sys.cpus().len();
        let cpu_usage_percent = sys.global_cpu_usage();
        let load = System::load_average();

        let total_memory_mb = sys.total_memory() / (1024 * 1024);
        let used_memory_mb = sys.used_memory() / (1024 * 1024);
        let free_memory_mb = sys.free_memory() / (1024 * 1024);
        let available_memory_mb = sys.available_memory() / (1024 * 1024);

        let pid = sysinfo::get_current_pid().ok();
        let process_rss_mb = pid
            .and_then(|p| sys.process(p))
            .map(|pr| pr.memory() / (1024 * 1024))
            .unwrap_or(0);

        SystemVitals {
            cpu_brand,
            cpu_cores_logical,
            cpu_usage_percent,
            load_avg_one: load.one,
            load_avg_five: load.five,
            load_avg_fifteen: load.fifteen,
            total_memory_mb,
            used_memory_mb,
            free_memory_mb,
            available_memory_mb,
            process_rss_mb,
        }
    }

    pub fn format_report(&self) -> String {
        format!(
            "System Vitals Metrics:\n\
             - CPU: {} ({} logical cores)\n\
             - CPU Usage: {:.1}%\n\
             - Load Average: 1m: {:.2}, 5m: {:.2}, 15m: {:.2}\n\
             - Memory: {} MB used / {} MB total (Free: {} MB, Available: {} MB)\n\
             - App Memory (RSS): {} MB",
            self.cpu_brand,
            self.cpu_cores_logical,
            self.cpu_usage_percent,
            self.load_avg_one,
            self.load_avg_five,
            self.load_avg_fifteen,
            self.used_memory_mb,
            self.total_memory_mb,
            self.free_memory_mb,
            self.available_memory_mb,
            self.process_rss_mb
        )
    }

    pub fn short_summary(&self) -> String {
        format!(
            "Load: {:.1}, Free RAM: {}MB",
            self.load_avg_one, self.available_memory_mb
        )
    }
}
