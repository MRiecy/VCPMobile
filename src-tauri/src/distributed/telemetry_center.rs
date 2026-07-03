// distributed/telemetry_center.rs
// Pull-based On-Demand hardware telemetries cache manager for VCP Distributed Node.
// Integrates JNI calls and sysfs/procfs I/O, sharing cached readings across tools.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::AppHandle;

#[derive(Clone)]
struct CpuSample {
    total: u64,
    idle: u64,
}

struct CacheItem<T> {
    data: T,
    last_updated: Option<Instant>,
    ttl: Duration,
}

impl<T: Default> CacheItem<T> {
    fn new(ttl_secs: u64) -> Self {
        Self {
            data: T::default(),
            last_updated: None,
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    fn get_valid(&self) -> Option<&T> {
        match self.last_updated {
            Some(t) if t.elapsed() < self.ttl => Some(&self.data),
            _ => None,
        }
    }

    fn update(&mut self, data: T) {
        self.data = data;
        self.last_updated = Some(Instant::now());
    }
}

pub struct TelemetryCenter {
    cpu_usage_cache: Mutex<CacheItem<String>>,
    cpu_temp_cache: Mutex<CacheItem<String>>,
    cpu_freq_cache: Mutex<CacheItem<String>>,
    gpu_cache: Mutex<CacheItem<String>>,
    battery_cache: Mutex<CacheItem<String>>,
    memory_cache: Mutex<CacheItem<String>>,
    storage_cache: Mutex<CacheItem<String>>,
    network_cache: Mutex<CacheItem<String>>,
    location_cache: Mutex<CacheItem<String>>,
    motion_cache: Mutex<CacheItem<String>>,
    ambient_cache: Mutex<CacheItem<String>>,

    // CPU delta calculator state
    cpu_prev_sample: Mutex<Option<CpuSample>>,
}

impl TelemetryCenter {
    pub fn new() -> Self {
        Self {
            cpu_usage_cache: Mutex::new(CacheItem::new(15)),
            cpu_temp_cache: Mutex::new(CacheItem::new(15)),
            cpu_freq_cache: Mutex::new(CacheItem::new(15)),
            gpu_cache: Mutex::new(CacheItem::new(30)),
            battery_cache: Mutex::new(CacheItem::new(30)),
            memory_cache: Mutex::new(CacheItem::new(15)),
            storage_cache: Mutex::new(CacheItem::new(120)),
            network_cache: Mutex::new(CacheItem::new(30)),
            location_cache: Mutex::new(CacheItem::new(120)),
            motion_cache: Mutex::new(CacheItem::new(15)),
            ambient_cache: Mutex::new(CacheItem::new(30)),

            cpu_prev_sample: Mutex::new(None),
        }
    }

    // =================================================================
    // Public Telemetries Pull Interface (Pull-based Throttling Cache)
    // =================================================================

    pub fn get_cpu_usage(&self, app: &AppHandle) -> String {
        let mut guard = self
            .cpu_usage_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_cpu_usage(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_cpu_temp(&self, app: &AppHandle) -> String {
        let mut guard = self
            .cpu_temp_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_cpu_temp(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_cpu_freq(&self) -> String {
        let mut guard = self
            .cpu_freq_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_cpu_freq();
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_gpu_info(&self, app: &AppHandle) -> String {
        let mut guard = self.gpu_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_gpu_info(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_battery_info(&self, app: &AppHandle) -> String {
        let mut guard = self.battery_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_battery_info(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_memory_info(&self) -> String {
        let mut guard = self.memory_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_memory_info();
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_storage_info(&self) -> String {
        let mut guard = self.storage_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_storage_info();
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_network_info(&self, app: &AppHandle) -> String {
        let mut guard = self.network_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_network_info(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_location_info(&self, app: &AppHandle) -> String {
        let mut guard = self
            .location_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_location_info(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_motion_info(&self, app: &AppHandle) -> String {
        let mut guard = self.motion_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_motion_info(app);
        guard.update(fresh.clone());
        fresh
    }

    pub fn get_ambient_info(&self, app: &AppHandle) -> String {
        let mut guard = self.ambient_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.get_valid() {
            return val.clone();
        }
        let fresh = self.capture_ambient_info(app);
        guard.update(fresh.clone());
        fresh
    }

    // =================================================================
    // Private JNI/IO Hardware Samplers
    // =================================================================

    fn capture_cpu_usage(&self, app: &AppHandle) -> String {
        let content = match crate::distributed::tools::sysfs_utils::execute_root_command_safe(
            app,
            "cat /proc/stat",
        ) {
            Some(out) => out,
            None => crate::distributed::tools::sysfs_utils::read_sysfs("/proc/stat"),
        };

        let mut current_sample = None;
        for line in content.lines() {
            if line.starts_with("cpu ") {
                let fields: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|s| s.parse::<u64>().ok())
                    .collect();
                if fields.len() >= 4 {
                    let total: u64 = fields.iter().sum();
                    let idle = fields[3] + fields.get(4).unwrap_or(&0);
                    current_sample = Some(CpuSample { total, idle });
                }
                break;
            }
        }

        match current_sample {
            Some(current) => {
                let mut prev = self
                    .cpu_prev_sample
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let result = match prev.as_ref() {
                    Some(prev_sample) => {
                        let total_diff = current.total.saturating_sub(prev_sample.total);
                        let idle_diff = current.idle.saturating_sub(prev_sample.idle);
                        if total_diff == 0 {
                            "0%".to_string()
                        } else {
                            let usage =
                                ((total_diff - idle_diff) as f64 / total_diff as f64) * 100.0;
                            format!("{:.0}%", usage)
                        }
                    }
                    None => "采集中...".to_string(),
                };
                *prev = Some(current);
                result
            }
            None => "受系统安全限制".to_string(),
        }
    }

    fn capture_cpu_temp(&self, app: &AppHandle) -> String {
        if let Some(raw_temp) = crate::distributed::tools::sysfs_utils::execute_root_command_safe(
            app,
            "cat /sys/class/thermal/thermal_zone0/temp",
        ) {
            if let Ok(temp_millideg) = raw_temp.trim().parse::<i64>() {
                if temp_millideg > 0 {
                    return format!("{}°C", temp_millideg / 1000);
                }
            }
        }

        #[cfg(target_os = "android")]
        {
            tauri_plugin_vcp_mobile::system::get_cpu_thermal_status(app.clone())
                .unwrap_or_else(|_| "N/A".to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            "正常".to_string()
        }
    }

    fn capture_cpu_freq(&self) -> String {
        let cpu_base = "/sys/devices/system/cpu";
        let mut freqs_khz: Vec<u64> = Vec::new();
        for i in 0..16 {
            let path = format!("{}/cpu{}/cpufreq/scaling_cur_freq", cpu_base, i);
            if let Some(freq) = crate::distributed::tools::sysfs_utils::read_sysfs_u64(&path) {
                freqs_khz.push(freq);
            }
        }

        if freqs_khz.is_empty() {
            return "N/A".to_string();
        }

        let max_freq = *freqs_khz.iter().max().unwrap();
        let min_freq = *freqs_khz.iter().min().unwrap();

        if max_freq > min_freq * 12 / 10 {
            format!(
                "{:.1}/{:.1}GHz",
                min_freq as f64 / 1_000_000.0,
                max_freq as f64 / 1_000_000.0,
            )
        } else {
            format!("{:.1}GHz", max_freq as f64 / 1_000_000.0)
        }
    }

    fn capture_gpu_info(&self, app: &AppHandle) -> String {
        let mut gpu_loaded = false;
        let mut gpu_load_str = "受系统安全限制".to_string();

        let gpu_renderer = tauri_plugin_vcp_mobile::system::get_gpu_status(app.clone())
            .map(|status| status.renderer)
            .unwrap_or_else(|_| "Unknown GPU".to_string());

        if let Some(raw_busy) = crate::distributed::tools::sysfs_utils::execute_root_command_safe(
            app,
            "cat /sys/class/kgsl/kgsl-3d0/gpubusy",
        ) {
            let raw_parts: Vec<&str> = raw_busy.split_whitespace().collect();
            if raw_parts.len() >= 2 {
                if let (Ok(busy), Ok(total)) =
                    (raw_parts[0].parse::<u64>(), raw_parts[1].parse::<u64>())
                {
                    if total > 0 {
                        let pct = (busy as f64 / total as f64) * 100.0;
                        gpu_load_str = format!("{}%", pct.round() as u64);
                        gpu_loaded = true;
                    }
                }
            }
        }

        if !gpu_loaded {
            let mali_paths = [
                "cat /sys/devices/platform/14ac0000.mali/utilization",
                "cat /sys/devices/platform/gpu/utilization",
                "cat /sys/devices/platform/mali/utilization",
            ];
            for path in &mali_paths {
                if let Some(raw_busy) =
                    crate::distributed::tools::sysfs_utils::execute_root_command_safe(app, path)
                {
                    let clean = raw_busy.trim().trim_end_matches('%');
                    if let Ok(load) = clean.parse::<u64>() {
                        gpu_load_str = format!("{}%", load);
                        break;
                    }
                }
            }
        }

        format!("型号: {} | 负载: {}", gpu_renderer, gpu_load_str)
    }

    fn capture_battery_info(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let status = tauri_plugin_vcp_mobile::system::get_battery_status(app.clone())?;
                let status_str = status.status.unwrap_or_else(|| "未知".to_string());
                let temp_str = match status.temperature {
                    Some(t) if t >= 0.0 => format!("{:.1}°C", t),
                    _ => "N/A".to_string(),
                };
                let pwr_save = if status.is_power_save_mode {
                    " (低功耗模式)"
                } else {
                    ""
                };
                Ok(format!(
                    "电量: {}%{} | 状态: {} | 温度: {}",
                    status.level, pwr_save, status_str, temp_str
                ))
            })();

            result.unwrap_or_else(|e| format!("电量: N/A | 错误: {}", e))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            "电池信息不可用".to_string()
        }
    }

    fn capture_memory_info(&self) -> String {
        let content = crate::distributed::tools::sysfs_utils::read_sysfs("/proc/meminfo");
        if content.is_empty() {
            return "内存信息不可用".to_string();
        }

        let mut mem_total = 0;
        let mut mem_available = 0;
        let mut swap_total = 0;
        let mut swap_free = 0;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let key = parts[0].trim_end_matches(':');
            let val: u64 = parts[1].parse().unwrap_or(0);

            match key {
                "MemTotal" => mem_total = val,
                "MemAvailable" => mem_available = val,
                "SwapTotal" => swap_total = val,
                "SwapFree" => swap_free = val,
                _ => {}
            }
        }

        let used_kb = mem_total.saturating_sub(mem_available);
        let usage_pct = if mem_total > 0 {
            (used_kb as f64 / mem_total as f64 * 100.0) as u64
        } else {
            0
        };

        let mut result = format!(
            "内存: {} / {} ({}%已用) | 可用: {}",
            crate::distributed::tools::sysfs_utils::format_kb(used_kb),
            crate::distributed::tools::sysfs_utils::format_kb(mem_total),
            usage_pct,
            crate::distributed::tools::sysfs_utils::format_kb(mem_available),
        );

        if swap_total > 0 {
            let swap_used = swap_total.saturating_sub(swap_free);
            result.push_str(&format!(
                " | Swap: {}/{}",
                crate::distributed::tools::sysfs_utils::format_kb(swap_used),
                crate::distributed::tools::sysfs_utils::format_kb(swap_total),
            ));
        }

        result
    }

    fn capture_storage_info(&self) -> String {
        #[cfg(unix)]
        unsafe {
            let path = std::ffi::CString::new("/data")
                .unwrap_or_else(|_| std::ffi::CString::new("/").unwrap());
            let mut stat: libc::statvfs = std::mem::zeroed();
            let ret = libc::statvfs(path.as_ptr(), &mut stat);
            if ret != 0 {
                return "存储信息不可用".to_string();
            }

            let block_size = stat.f_frsize as u64;
            let total = stat.f_blocks as u64 * block_size;
            let available = stat.f_bavail as u64 * block_size;
            let used = total.saturating_sub(available);

            let usage_pct = if total > 0 {
                (used as f64 / total as f64 * 100.0) as u64
            } else {
                0
            };

            format!(
                "内部存储: {} / {} ({}%已用)",
                crate::distributed::tools::sysfs_utils::format_bytes(used),
                crate::distributed::tools::sysfs_utils::format_bytes(total),
                usage_pct,
            )
        }
        #[cfg(not(unix))]
        {
            "存储信息不可用(非Unix平台)".to_string()
        }
    }

    fn capture_network_info(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let status = tauri_plugin_vcp_mobile::system::get_network_status(app.clone())?;
                if !status.connected {
                    return Ok("网络: 未连接".to_string());
                }

                let format_speed = |kbps: i32| -> String {
                    if kbps >= 1000 {
                        format!("{:.1}Mbps", kbps as f64 / 1000.0)
                    } else {
                        format!("{}Kbps", kbps)
                    }
                };
                let down_str = format_speed(status.down_speed_kbps);
                let up_str = format_speed(status.up_speed_kbps);

                Ok(format!(
                    "类型: {} | IP: {} | 下行: {} | 上行: {}",
                    status.r#type, status.ip, down_str, up_str
                ))
            })();

            result.unwrap_or_else(|e| format!("网络: 错误 ({})", e))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            "网络: 未连接".to_string()
        }
    }

    fn capture_location_info(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let data = tauri_plugin_vcp_mobile::system::get_sensor_data(
                    app.clone(),
                    "location".to_string(),
                )?;
                let val = data
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(val)
            })();
            result.unwrap_or_else(|e| format!("定位信息采集失败: {}", e))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            "坐标: 39.9000°N, 116.4000°E | 精度: 15m | 海拔: 50m (模拟)".to_string()
        }
    }

    fn capture_motion_info(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let data = tauri_plugin_vcp_mobile::system::get_sensor_data(
                    app.clone(),
                    "motion".to_string(),
                )?;
                let val = data
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(val)
            })();
            result.unwrap_or_else(|e| format!("运动传感器采集失败: {}", e))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let brief = "状态: 静止 (模拟)";
            let detail = "状态: 静止 | 平均加速度: 9.80m/s² (峰值: 9.80m/s²) (模拟)";
            format!(
                "[===vcp_fold: 0.0 ::desc: 物理运动姿态粗略状态(静止、步行、步行中或剧烈移动)===]\n{}\n\n[===vcp_fold: 0.50 ::desc: 九轴高频遥测指标、旋转角速度、加速度峰值、三轴磁敏度物理强度===]\n{}",
                brief, detail
            )
        }
    }

    fn capture_ambient_info(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let data = tauri_plugin_vcp_mobile::system::get_sensor_data(
                    app.clone(),
                    "ambient".to_string(),
                )?;
                let val = data
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(val)
            })();
            result.unwrap_or_else(|e| format!("环境传感器采集失败: {}", e))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let brief = "环境光: 150 lux (室内) (模拟)";
            let detail = "环境光: 150 lux (室内) | 气压: 1013 hPa (模拟)";
            format!(
                "[===vcp_fold: 0.0 ::desc: 当前所处的物理环境光照度大体描述(如暗、室内、户外)===]\n{}\n\n[===vcp_fold: 0.45 ::desc: 物理环境大气压强、精确光照度数值与场景气压监测===]\n{}",
                brief, detail
            )
        }
    }

    // =================================================================
    // Device Summary Telemetries Helper Formatter
    // =================================================================

    pub fn get_memory_brief(&self) -> String {
        let content = crate::distributed::tools::sysfs_utils::read_sysfs("/proc/meminfo");
        if content.is_empty() {
            return "内存:N/A".to_string();
        }
        let mut total = 0u64;
        let mut avail = 0u64;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                avail = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
        if total > 0 {
            let pct = ((total - avail) * 100).checked_div(total).unwrap_or(0);
            format!("内存:{}%", pct)
        } else {
            "内存:N/A".to_string()
        }
    }

    pub fn get_battery_brief(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let status = tauri_plugin_vcp_mobile::system::get_battery_status(app.clone())?;
                let suffix = match status.status.as_deref() {
                    Some("充电中") => "充电中",
                    Some("已充满") => "已满",
                    _ => "",
                };
                Ok(format!("电量:{}%{}", status.level, suffix))
            })();
            result.unwrap_or_else(|_| "电量:N/A".to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let cap = crate::distributed::tools::sysfs_utils::read_sysfs(
                "/sys/class/power_supply/battery/capacity",
            );
            let status = crate::distributed::tools::sysfs_utils::read_sysfs(
                "/sys/class/power_supply/battery/status",
            );
            if cap.is_empty() {
                return "电量:N/A".to_string();
            }
            let suffix = match status.as_str() {
                "Charging" => "充电中",
                "Full" => "已满",
                _ => "",
            };
            if suffix.is_empty() {
                format!("电量:{}%", cap.trim())
            } else {
                format!("电量:{}%{}", cap.trim(), suffix)
            }
        }
    }

    pub fn get_network_brief(&self, app: &AppHandle) -> String {
        #[cfg(target_os = "android")]
        {
            let result: Result<String, String> = (|| {
                let status = tauri_plugin_vcp_mobile::system::get_network_status(app.clone())?;
                if status.connected {
                    Ok(status.r#type)
                } else {
                    Ok("离线".to_string())
                }
            })();
            result.unwrap_or_else(|_| "离线".to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            let route = crate::distributed::tools::sysfs_utils::read_sysfs("/proc/net/route");
            for line in route.lines().skip(1) {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() >= 2 && cols[1] == "00000000" {
                    let iface = cols[0];
                    let net_type = if iface.starts_with("wlan") {
                        "WiFi"
                    } else if iface.starts_with("rmnet") || iface.starts_with("ccmni") {
                        "移动数据"
                    } else {
                        iface
                    };
                    return net_type.to_string();
                }
            }
            "离线".to_string()
        }
    }

    pub fn get_coords_brief(&self, app: &AppHandle) -> String {
        let loc = self.get_location_info(app);
        if loc.starts_with("坐标:") {
            if let Some(c) = loc.split(" | ").next() {
                return c.to_string();
            }
        }
        "坐标: N/A".to_string()
    }

    pub fn get_motion_brief(&self, app: &AppHandle) -> String {
        let motion = self.get_motion_info(app);
        if motion.starts_with("状态: ") {
            if let Some(m) = motion
                .strip_prefix("状态: ")
                .and_then(|s| s.split(" | ").next())
            {
                return m.to_string();
            }
        }
        "静止".to_string()
    }
}
