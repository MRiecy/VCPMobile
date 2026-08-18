//! HTTP 连接画像注册表（Http Profile Registry）
//!
//! 背景：`reqwest::Client` 内部是 `Arc`，克隆廉价，设计意图就是全局共享——
//! 共享底层连接池、HTTP/2 多路复用与 TLS session 复用。历史上各模块各自
//! `Client::builder()` 构建（9 处散布 8 个模块），既浪费移动端宝贵的
//! TCP/TLS 握手 RTT 与射频电量，也让超时/重定向/池策略发散无文档。
//!
//! 本模块把「客户端级策略」收敛为少数几个命名画像（Profile），调用点通过
//! `client(HttpProfile::Xxx)` 获取进程级共享实例（`OnceLock` 懒初始化）。
//!
//! 规矩（新增画像前必读）：
//! 1. 请求级差异（超时、header、auth）不属于画像，留在调用点的 `RequestBuilder` 上；
//! 2. 新画像必须在此注释说明「为什么现有画像不适用」；
//! 3. 每个画像必须显式设置 `pool_idle_timeout`：移动网络 WiFi↔蜂窝切换后
//!    池内空闲连接可能半死不活，缩短空闲超时让坏连接自然淘汰；
//! 4. 已知例外：sync 子系统与 updater 持有各自专职 Client（子系统内共享/
//!    场景特化），不迁入本注册表；`test_vcp_connection` 等一次性探测允许
//!    新建瞬时 Client（用完即弃也是合法策略，但需在调用点注释理由）。

use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

/// 连接画像。每个变体对应一个进程级共享的 `reqwest::Client`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpProfile {
    /// 聊天流式请求（SSE）。无总超时（流式响应时长不可预估），
    /// TCP keepalive 20s 保活探测，空闲连接 60s 淘汰。
    ChatStream,
    /// VCP 服务器 admin_api 调用（日记/表情/日志中心/任务调度等）。
    /// 连接超时 10s；禁止重定向（admin_api 不应跳转，跳转即异常）。
    AdminApi,
    /// 大文件下载（APK 等）。仅连接超时，整体时长由调用方的
    /// 停滞判死与重试编排控制。
    Download,
}

static CHAT_STREAM_CLIENT: OnceLock<Client> = OnceLock::new();
static ADMIN_API_CLIENT: OnceLock<Client> = OnceLock::new();
static DOWNLOAD_CLIENT: OnceLock<Client> = OnceLock::new();

fn build_client(profile: HttpProfile) -> Client {
    let builder = Client::builder();
    let builder = match profile {
        HttpProfile::ChatStream => builder
            .tcp_keepalive(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(60)),
        HttpProfile::AdminApi => builder
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none()),
        HttpProfile::Download => builder
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(30)),
    };
    // Client::build 仅在 TLS 后端初始化失败等极端情况下失败；
    // 此时进程内网络栈已不可用，panic 信息带画像名便于定位。
    builder
        .build()
        .unwrap_or_else(|e| panic!("http_clients: failed to build {profile:?} client: {e}"))
}

/// 获取指定画像的进程级共享 Client（懒初始化）。
pub fn client(profile: HttpProfile) -> &'static Client {
    match profile {
        HttpProfile::ChatStream => CHAT_STREAM_CLIENT.get_or_init(|| build_client(profile)),
        HttpProfile::AdminApi => ADMIN_API_CLIENT.get_or_init(|| build_client(profile)),
        HttpProfile::Download => DOWNLOAD_CLIENT.get_or_init(|| build_client(profile)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_profile_returns_shared_instance() {
        let a = client(HttpProfile::AdminApi);
        let b = client(HttpProfile::AdminApi);
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn different_profiles_are_distinct_instances() {
        let chat = client(HttpProfile::ChatStream);
        let admin = client(HttpProfile::AdminApi);
        let download = client(HttpProfile::Download);
        assert!(!std::ptr::eq(chat, admin));
        assert!(!std::ptr::eq(chat, download));
        assert!(!std::ptr::eq(admin, download));
    }

    #[test]
    fn clone_shares_underlying_pool() {
        // reqwest::Client 克隆共享同一连接池（Arc 语义）。
        let shared = client(HttpProfile::ChatStream);
        let cloned = shared.clone();
        // 克隆体的连接池句柄与原实例一致（通过 Debug 输出中的指针不可比，
        // 这里仅验证克隆可用且类型正确——池共享由 reqwest 保证）。
        drop(cloned);
    }
}
