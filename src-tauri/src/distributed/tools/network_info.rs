// distributed/tools/network_info.rs
// [Streaming] MobileNetworkInfo — network type, IP, traffic stats.
// Reads /proc/net/route, /sys/class/net/*/statistics/, /sys/class/net/*/operstate

use serde_json::json;

use crate::distributed::tool_registry::StreamingTool;
use crate::distributed::types::ToolManifest;

use super::sysfs_utils::{format_bytes, read_sysfs, read_sysfs_u64};

pub struct NetworkInfoTool;

impl NetworkInfoTool {
    /// Find the default network interface from /proc/net/route.
    fn find_default_iface(&self) -> Option<String> {
        let content = read_sysfs("/proc/net/route");
        for line in content.lines().skip(1) {
            // skip header
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 2 && fields[1] == "00000000" {
                // Destination 0.0.0.0 = default route
                return Some(fields[0].to_string());
            }
        }
        None
    }

    /// Detect network type from interface name.
    fn detect_type(&self, iface: &str) -> String {
        if iface.starts_with("wlan") || iface.starts_with("wifi") {
            "WiFi".to_string()
        } else if iface.starts_with("rmnet") || iface.starts_with("ccmni") {
            "移动数据".to_string()
        } else if iface.starts_with("eth") {
            "以太网".to_string()
        } else if iface.starts_with("lo") {
            "回环".to_string()
        } else {
            iface.to_string()
        }
    }

    /// Read IP address from /proc/net/fib_trie or fallback to interface info.
    /// This is a simplified approach — reads from /proc/net/if_inet6 or fib_trie.
    fn read_ip(&self, iface: &str) -> String {
        // Try reading from /proc/net/fib_trie (complex) — simplified: use operstate check
        // For a more robust solution, Phase 2 can use frontend navigator.connection
        let operstate = read_sysfs(&format!("/sys/class/net/{}/operstate", iface));
        if operstate == "up" || operstate == "unknown" {
            // Try to get IP from /proc/net/fib_trie
            if let Some(ip) = self.parse_fib_trie_ip(iface) {
                return ip;
            }
            return "已连接".to_string();
        }
        "未连接".to_string()
    }

    /// Parse /proc/net/fib_trie to extract IP for a given interface.
    /// This is best-effort; Android may restrict access.
    fn parse_fib_trie_ip(&self, _iface: &str) -> Option<String> {
        let content = read_sysfs("/proc/net/fib_trie");
        if content.is_empty() {
            return None;
        }

        // Look for /32 host routes which are local IPs
        // Format: "  |-- X.X.X.X" followed by "/32 host LOCAL"
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.contains("/32 host LOCAL") {
                // The IP is in the previous line
                if i > 0 {
                    let prev = lines[i - 1].trim();
                    if prev.starts_with("|-- ") {
                        let ip = prev.trim_start_matches("|-- ");
                        // Skip loopback
                        if !ip.starts_with("127.") {
                            return Some(ip.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Read traffic statistics for an interface.
    fn read_traffic(&self, iface: &str) -> String {
        let rx = read_sysfs_u64(&format!("/sys/class/net/{}/statistics/rx_bytes", iface));
        let tx = read_sysfs_u64(&format!("/sys/class/net/{}/statistics/tx_bytes", iface));

        match (rx, tx) {
            (Some(r), Some(t)) => {
                format!("接收: {} 发送: {}", format_bytes(r), format_bytes(t))
            }
            _ => String::new(),
        }
    }
}

impl StreamingTool for NetworkInfoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "MobileNetworkInfo".to_string(),
            description: "移动设备网络状态(类型/IP/流量)".to_string(),
            parameters: json!({}),
            tool_type: "mobile".to_string(),
        }
    }

    fn placeholder_key(&self) -> &str {
        "{{MobileNetwork}}"
    }

    fn poll_interval_secs(&self) -> u64 {
        30
    }

    fn read_current(&self) -> Result<String, String> {
        let iface = match self.find_default_iface() {
            Some(i) => i,
            None => return Ok("网络: 未连接".to_string()),
        };

        let net_type = self.detect_type(&iface);
        let ip = self.read_ip(&iface);
        let traffic = self.read_traffic(&iface);

        let mut result = format!("类型: {} | IP: {}", net_type, ip);
        if !traffic.is_empty() {
            result.push_str(&format!(" | {}", traffic));
        }

        Ok(result)
    }
}
