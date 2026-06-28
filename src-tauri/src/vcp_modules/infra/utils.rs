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

/// 规范化 VCP 服务器 URL。
/// 优先检查是否为合规格式，若非合规格式则直接提取 scheme://host:port 部分并统一拼接后缀。
/// 对于非法或缺失 Scheme 的 URL，不做额外处理，直接返回原样。
pub fn normalize_vcp_url(url_str: &str) -> String {
    let cleaned = url_str.trim();
    if let Ok(mut url) = url::Url::parse(cleaned) {
        let path = url.path().trim_end_matches('/').to_string();

        // 1. 先看看是否是需要的格式（以 /v1/chat/completions 或 /chat/completions 结尾）
        if path.ends_with("/v1/chat/completions") || path.ends_with("/chat/completions") {
            url.set_path(&path);
            return url.to_string();
        }

        // 2. 如果不是，直接获取 scheme://host:port 部分，后面的丢掉，我们自己拼接
        let scheme = url.scheme();
        let host = url.host_str().unwrap_or("");
        let port = match url.port() {
            Some(p) => format!(":{}", p),
            None => "".to_string(),
        };

        return format!("{}://{}{}/v1/chat/completions", scheme, host, port);
    }

    // 解析失败（如缺失 scheme），不做额外处理，直接返回原样
    url_str.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_vcp_url() {
        assert_eq!(normalize_vcp_url("127.0.0.1:8000"), "127.0.0.1:8000");
        assert_eq!(
            normalize_vcp_url("http://127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            normalize_vcp_url("http://127.0.0.1:8080/v1/chat/"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            normalize_vcp_url("http://127.0.0.1:8080/v1/chat/completions/"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            normalize_vcp_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_vcp_url("http://127.0.0.1/proxy/"),
            "http://127.0.0.1/v1/chat/completions"
        );
        assert_eq!(
            normalize_vcp_url("  http://localhost:3000/  "),
            "http://localhost:3000/v1/chat/completions"
        );
    }
}
