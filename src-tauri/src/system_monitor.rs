use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{Components, Disks, Networks, System};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemMetrics {
    pub timestamp: u64,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub network: IoRate,
    pub disk: DiskMetrics,
    pub temperatures: Vec<TemperatureSensor>,
    pub fans: Vec<FanSensor>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpuMetrics {
    pub usage_percent: f32,
    pub frequency_mhz: u64,
    pub logical_processors: usize,
    pub temperature_c: Option<f32>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IoRate {
    pub down_bytes_per_sec: u64,
    pub up_bytes_per_sec: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiskMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub usage_percent: f32,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TemperatureSensor {
    pub name: String,
    pub celsius: f32,
    pub source: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FanSensor {
    pub name: String,
    pub rpm: u64,
    pub source: String,
}

pub(crate) struct SystemMonitorSampler {
    system: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    last_sample: Instant,
    last_metrics: Option<SystemMetrics>,
}

impl SystemMonitorSampler {
    pub(crate) fn new() -> Self {
        Self {
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            last_sample: Instant::now(),
            last_metrics: None,
        }
    }

    pub(crate) fn sample(&mut self) -> Result<SystemMetrics, String> {
        // panel 与 popup 可能在同一时刻请求。短时间内复用同一快照，避免第二次刷新把速率间隔压到几毫秒。
        if self.last_sample.elapsed() < Duration::from_millis(250) {
            if let Some(metrics) = &self.last_metrics {
                return Ok(metrics.clone());
            }
        }
        let elapsed = self.last_sample.elapsed().as_secs_f64().max(0.001);
        self.last_sample = Instant::now();

        self.system.refresh_cpu_all();
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.disks.refresh(true);
        self.components.refresh(false);

        let cpu_usage = self.system.global_cpu_usage().clamp(0.0, 100.0);
        let cpu_frequency = if self.system.cpus().is_empty() {
            0
        } else {
            self.system
                .cpus()
                .iter()
                .map(|cpu| cpu.frequency())
                .sum::<u64>()
                / self.system.cpus().len() as u64
        };

        let total_memory = self.system.total_memory();
        let used_memory = self.system.used_memory();
        let available_memory = self.system.available_memory();
        let memory_percent = percent(used_memory, total_memory);

        let down_delta = self.networks.iter().map(|(_, item)| item.received()).sum::<u64>();
        let up_delta = self.networks.iter().map(|(_, item)| item.transmitted()).sum::<u64>();

        let mut disk_total = 0_u64;
        let mut disk_available = 0_u64;
        let mut disk_read_delta = 0_u64;
        let mut disk_write_delta = 0_u64;
        for disk in self.disks.list() {
            disk_total = disk_total.saturating_add(disk.total_space());
            disk_available = disk_available.saturating_add(disk.available_space());
            let usage = disk.usage();
            disk_read_delta = disk_read_delta.saturating_add(usage.read_bytes);
            disk_write_delta = disk_write_delta.saturating_add(usage.written_bytes);
        }
        let disk_used = disk_total.saturating_sub(disk_available);

        let temperatures = self
            .components
            .iter()
            .filter_map(|component| {
                component.temperature().and_then(|value| {
                    value.is_finite().then(|| TemperatureSensor {
                        name: component.label().to_string(),
                        celsius: value,
                        source: "sysinfo".to_string(),
                    })
                })
            })
            .collect::<Vec<_>>();

        let metrics = SystemMetrics {
            timestamp: unix_seconds(),
            cpu: CpuMetrics {
                usage_percent: cpu_usage,
                frequency_mhz: cpu_frequency,
                logical_processors: self.system.cpus().len(),
                temperature_c: choose_cpu_temperature(&temperatures),
            },
            memory: MemoryMetrics {
                total_bytes: total_memory,
                used_bytes: used_memory,
                available_bytes: available_memory,
                usage_percent: memory_percent,
            },
            network: IoRate {
                down_bytes_per_sec: rate(down_delta, elapsed),
                up_bytes_per_sec: rate(up_delta, elapsed),
            },
            disk: DiskMetrics {
                total_bytes: disk_total,
                used_bytes: disk_used,
                usage_percent: percent(disk_used, disk_total),
                read_bytes_per_sec: rate(disk_read_delta, elapsed),
                write_bytes_per_sec: rate(disk_write_delta, elapsed),
            },
            temperatures,
            // sysinfo 当前没有跨平台风扇 RPM API。为了避免额外 WMI/windows-core
            // 依赖冲突，这里保持为空；后续可单独实现无第三方 WMI crate 的 Windows 传感器层。
            fans: Vec::new(),
        };
        self.last_metrics = Some(metrics.clone());
        Ok(metrics)
    }
}

fn rate(bytes: u64, elapsed: f64) -> u64 {
    if elapsed <= 0.0 {
        0
    } else {
        (bytes as f64 / elapsed).round().max(0.0) as u64
    }
}

fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn valid_temperature(sensor: &TemperatureSensor) -> bool {
    sensor.celsius.is_finite() && sensor.celsius > -40.0 && sensor.celsius < 160.0
}

fn choose_cpu_temperature(temperatures: &[TemperatureSensor]) -> Option<f32> {
    temperatures
        .iter()
        .filter(|sensor| valid_temperature(sensor))
        .filter(|sensor| {
            let name = sensor.name.to_ascii_lowercase();
            name.contains("cpu") || name.contains("package") || name.contains("core") || name.contains("processor")
        })
        .map(|sensor| sensor.celsius)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}
