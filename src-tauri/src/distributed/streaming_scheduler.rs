use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::time::{self, Instant};
use tokio_util::sync::CancellationToken;

use super::tool_registry::StreamingTool;
use super::DistributedState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorDemand {
    pub location: bool,
    pub motion: bool,
    pub ambient: bool,
}

impl SensorDemand {
    pub const LOCATION: Self = Self {
        location: true,
        motion: false,
        ambient: false,
    };
    pub const MOTION: Self = Self {
        location: false,
        motion: true,
        ambient: false,
    };
    pub const AMBIENT: Self = Self {
        location: false,
        motion: false,
        ambient: true,
    };

    pub fn union(self, other: Self) -> Self {
        Self {
            location: self.location || other.location,
            motion: self.motion || other.motion,
            ambient: self.ambient || other.ambient,
        }
    }

    pub fn is_empty(self) -> bool {
        !self.location && !self.motion && !self.ambient
    }
}

#[derive(Clone)]
pub struct StreamingToolSpec {
    pub name: String,
    pub placeholder: String,
    pub interval: Duration,
    pub sensor_demand: SensorDemand,
    pub tool: Arc<dyn StreamingTool>,
}

#[derive(Clone, Default)]
pub struct StreamingPlan {
    pub tools: Vec<StreamingToolSpec>,
}

impl StreamingPlan {
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

struct ScheduledTool {
    spec: StreamingToolSpec,
    next_due: Instant,
}

/// 单个连接会话内的流式工具调度器。
///
/// 授权计划在会话建立时冻结；工具配置变化会结束旧会话并以新计划重连。因此这里不监听
/// 可变注册表，也不会在零 streaming tool 时创建空转定时器。
pub struct StreamingScheduler {
    tools: Vec<ScheduledTool>,
}

impl StreamingScheduler {
    pub fn new(plan: StreamingPlan) -> Option<Self> {
        if plan.is_empty() {
            return None;
        }
        let now = Instant::now();
        Some(Self {
            tools: plan
                .tools
                .into_iter()
                .map(|spec| ScheduledTool {
                    spec,
                    next_due: now,
                })
                .collect(),
        })
    }

    fn next_deadline(&self) -> Instant {
        self.tools
            .iter()
            .map(|tool| tool.next_due)
            .min()
            .unwrap_or_else(Instant::now)
    }

    fn take_due(&mut self, now: Instant) -> Vec<StreamingToolSpec> {
        let mut due = Vec::new();
        for scheduled in &mut self.tools {
            if scheduled.next_due > now {
                continue;
            }
            due.push(scheduled.spec.clone());
            let interval = scheduled.spec.interval.max(Duration::from_secs(1));
            while scheduled.next_due <= now {
                scheduled.next_due += interval;
            }
        }
        due
    }

    pub async fn run<F, Fut>(
        mut self,
        app: AppHandle,
        cancel_token: CancellationToken,
        mut publish: F,
    ) where
        F: FnMut(HashMap<String, String>) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        loop {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => break,
                _ = time::sleep_until(self.next_deadline()) => {}
            }

            let due = self.take_due(Instant::now());
            if due.is_empty() {
                continue;
            }

            let demand = due.iter().fold(SensorDemand::default(), |demand, tool| {
                demand.union(tool.sensor_demand)
            });
            let tag = format!("distributed:telemetry:{}", uuid::Uuid::new_v4());
            let _lease = StreamingForegroundLease::acquire(&app, tag);

            if !demand.is_empty() {
                match tauri_plugin_vcp_mobile::system::sample_sensor_data(
                    app.clone(),
                    demand.location,
                    demand.motion,
                    demand.ambient,
                )
                .await
                {
                    Ok(snapshot) => app
                        .state::<DistributedState>()
                        .telemetry
                        .update_sensor_snapshot(&snapshot),
                    Err(error) => {
                        log::warn!("[Distributed] Native sensor sample failed: {error}")
                    }
                }
            }

            let render_app = app.clone();
            let placeholders = match tauri::async_runtime::spawn_blocking(move || {
                let mut values = HashMap::with_capacity(due.len());
                for spec in due {
                    match spec.tool.read_current(&render_app) {
                        Ok(value) => {
                            values.insert(spec.placeholder, value);
                        }
                        Err(error) => log::warn!(
                            "[Distributed] Streaming tool '{}' sample failed: {}",
                            spec.name,
                            error
                        ),
                    }
                }
                values
            })
            .await
            {
                Ok(values) => values,
                Err(error) => {
                    log::warn!("[Distributed] Streaming batch task failed: {error}");
                    continue;
                }
            };

            if placeholders.is_empty() {
                continue;
            }
            if let Err(error) = publish(placeholders).await {
                log::warn!("[Distributed] Streaming placeholder publish failed: {error}");
                break;
            }
        }
    }
}

struct StreamingForegroundLease {
    app: AppHandle,
    tag: String,
}

impl StreamingForegroundLease {
    fn acquire(app: &AppHandle, tag: String) -> Self {
        acquire_foreground(app, &tag);
        Self {
            app: app.clone(),
            tag,
        }
    }
}

impl Drop for StreamingForegroundLease {
    fn drop(&mut self) {
        release_foreground(&self.app, &self.tag);
    }
}

#[cfg(target_os = "android")]
fn acquire_foreground(app: &AppHandle, tag: &str) {
    if let Err(error) = tauri_plugin_vcp_mobile::stream::acquire_foreground_inner(
        app,
        tag,
        10,
        "distributed",
        false,
    ) {
        log::warn!("[Distributed] Failed to acquire telemetry foreground lease: {error}");
    }
}

#[cfg(not(target_os = "android"))]
fn acquire_foreground(_app: &AppHandle, _tag: &str) {}

#[cfg(target_os = "android")]
fn release_foreground(app: &AppHandle, tag: &str) {
    if let Err(error) = tauri_plugin_vcp_mobile::stream::release_foreground_inner(app, tag) {
        log::warn!("[Distributed] Failed to release telemetry foreground lease: {error}");
    }
}

#[cfg(not(target_os = "android"))]
fn release_foreground(_app: &AppHandle, _tag: &str) {}
