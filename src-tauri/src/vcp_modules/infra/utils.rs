// utils.rs - 基础设施层共享的无状态通用原语工具包
// 职责：沉淀纯算法级、无状态的高复用底层工具，面向全后端模块提供跨领域共享。

use sha2::{Digest, Sha256};

/// 协作式 CPU 挂起出让计数器 (YieldCounter)
/// 在重 I/O 或超大循环遍历中，用于每隔特定阈值自动挂起并出让当前 CPU 时间片，保障前台 WebView 帧率。
pub struct YieldCounter {
    count: u32,
    threshold: u32,
}

impl YieldCounter {
    /// 创建一个新的协作出让挂起计数器，指定出让阈值（默认推荐 150 - 200）
    pub fn new(threshold: u32) -> Self {
        Self {
            count: 0,
            threshold,
        }
    }

    /// 推进计数，并在达到阈值时自动挂起出让当前 CPU 时间片
    #[inline]
    pub async fn tick(&mut self) {
        self.count += 1;
        if self.count >= self.threshold {
            self.count = 0;
            tokio::task::yield_now().await;
        }
    }
}

/// 校验字符串是否为合法的 Content-Addressable Storage (CAS) 的 64位 SHA-256 哈希指纹
#[inline]
pub fn is_valid_cas_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// 获取当前系统秒级时间戳 (UNIX EPOCH)，防时钟回拨 panic 自愈
#[inline]
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// 获取当前系统毫秒级时间戳 (UNIX EPOCH)，防时钟回拨 panic 自愈
#[inline]
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// 按 VCPChat 的实体 ID 规则，将显示名称映射为可移植的 ASCII 路径段。
///
/// 桌面端使用 JavaScript UTF-16 正则逐 code unit 将非 `[a-zA-Z0-9_-]`
/// 字符替换为下划线；这里保留相同语义，避免 Android 创建出桌面端无法接收的 Owner ID。
pub(crate) fn desktop_compatible_id_base(name: &str) -> String {
    name.encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') => {
                char::from(byte)
            }
            _ => '_',
        })
        .collect()
}

/// 计算单字节切片的标准 SHA-256 十六进制摘要字串（统一小写输出）
#[inline]
pub fn calculate_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    finalize_sha256_hex(hasher)
}

/// 将已完成输入的 SHA-256 状态编码为固定 64 位小写十六进制字串。
///
/// 业务层继续负责输入顺序、分隔符和长度前缀；这里只统一摘要的最终文本编码。
#[inline]
pub fn finalize_sha256_hex(hasher: Sha256) -> String {
    hex::encode(hasher.finalize())
}

/// 后台延迟任务计时器工具
/// 传入延时时长、取消令牌，以及一个在未被取消且到期时执行的闭包
pub fn spawn_linger_task<F, Fut>(
    delay: std::time::Duration,
    cancel_token: tokio_util::sync::CancellationToken,
    action: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                // 被取消，优雅退出
            }
            _ = tokio::time::sleep(delay) => {
                // 时间到，且未被取消，执行操作
                action().await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_helpers_preserve_standard_lowercase_hex_contract() {
        assert_eq!(
            calculate_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            calculate_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(finalize_sha256_hex(hasher), calculate_sha256(b"abc"));
    }

    #[test]
    fn desktop_compatible_id_base_matches_vchat_ascii_replacement() {
        assert_eq!(desktop_compatible_id_base("Agent-01_ok"), "Agent-01_ok");
        assert_eq!(desktop_compatible_id_base("测试1"), "__1");
        assert_eq!(desktop_compatible_id_base("A B/😀"), "A_B___");
    }
}
