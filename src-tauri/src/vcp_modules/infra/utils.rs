// utils.rs - 基础设施层共享的无状态通用原语工具包
// 职责：沉淀纯算法级、无状态的高复用底层工具，面向全后端模块提供跨领域共享。

/// 协作式 CPU 挂起出让计数器 (YieldCounter)
/// 在重 I/O 或超大循环遍历中，用于每隔特定阈值自动挂起并出让当前 CPU 时间片，保障前台 WebView 帧率。
pub struct YieldCounter {
    count: u32,
    threshold: u32,
}

impl YieldCounter {
    /// 创建一个新的协作出让挂起计数器，指定出让阈值（默认推荐 150 - 200）
    pub fn new(threshold: u32) -> Self {
        Self { count: 0, threshold }
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

/// 计算多个字节切片（不连续数据段）的标准 SHA-256 十六进制摘要字串（统一小写输出）
pub fn calculate_sha256_slices(slices: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for slice in slices {
        hasher.update(slice);
    }
    hex::encode(hasher.finalize())
}

/// 计算单字节切片的标准 SHA-256 十六进制摘要字串（统一小写输出）
#[inline]
pub fn calculate_sha256(bytes: &[u8]) -> String {
    calculate_sha256_slices(&[bytes])
}
