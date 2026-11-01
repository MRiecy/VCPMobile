use super::lifecycle_state::LifecycleState;
use std::future::pending;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::watch;
use tokio::time::{sleep, sleep_until, Instant};

const HEARTBEAT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(super) struct WsActivity {
    foreground: bool,
    chats: usize,
    pub(super) resume_epoch: u64,
}

impl WsActivity {
    pub(super) fn maintain(self) -> bool {
        self.foreground || self.chats != 0
    }

    fn set_foreground(&mut self, foreground: bool) {
        if foreground && !self.foreground {
            self.resume_epoch = self.resume_epoch.wrapping_add(1);
        }
        self.foreground = foreground;
    }
}

#[derive(Clone)]
pub(super) struct WsActivityPolicy(watch::Sender<WsActivity>);

impl WsActivityPolicy {
    pub(super) fn new(foreground: bool) -> Self {
        let (sender, _) = watch::channel(WsActivity {
            foreground,
            chats: 0,
            resume_epoch: 0,
        });
        Self(sender)
    }

    pub(super) fn is_foreground(&self) -> bool {
        self.0.borrow().foreground
    }

    pub(super) fn set_foreground(&self, foreground: bool) -> bool {
        let mut changed = false;
        self.update(|state| {
            changed = foreground != state.foreground;
            state.set_foreground(foreground);
        });
        changed
    }

    fn update(&self, change: impl FnOnce(&mut WsActivity)) {
        // Always retain the current inputs, but wake the sockets only on a
        // change of demand or an actual foreground transition.
        self.0.send_if_modified(|state| {
            let previous = *state;
            change(state);
            state.maintain() != previous.maintain() || state.resume_epoch != previous.resume_epoch
        });
    }
}

pub(super) fn subscribe<R: Runtime>(app: &AppHandle<R>) -> watch::Receiver<WsActivity> {
    app.state::<LifecycleState>().ws_activity.0.subscribe()
}

#[must_use]
pub(crate) struct ChatActivityGuard(WsActivityPolicy);

impl ChatActivityGuard {
    pub(crate) fn acquire<R: Runtime>(app: &AppHandle<R>) -> Self {
        let policy = app.state::<LifecycleState>().ws_activity.clone();
        policy.update(|state| state.chats += 1);
        Self(policy)
    }
}

impl Drop for ChatActivityGuard {
    fn drop(&mut self) {
        self.0.update(|state| state.chats -= 1);
    }
}

pub(super) async fn until_idle(activity: &mut watch::Receiver<WsActivity>) {
    let _ = activity.wait_for(|state| !state.maintain()).await;
}

pub(super) async fn wait_retry<T>(
    endpoint: &mut watch::Receiver<T>,
    activity: &mut watch::Receiver<WsActivity>,
    delay: &mut Duration,
) {
    if activity.borrow().maintain() {
        tokio::select! {
            biased;
            _ = endpoint.changed() => {},
            _ = activity.changed() => {},
            _ = sleep(*delay) => {
                *delay = (*delay * 2).min(Duration::from_secs(60));
                return;
            }
        }
    }
    *delay = Duration::from_secs(1);
}

pub(super) enum HeartbeatAction {
    Ping(Vec<u8>),
    ProbeExpired,
}

#[derive(Clone, Copy)]
enum Timer {
    Idle,
    PingAt(Instant),
    ProbeNow,
    AwaitPong(Instant),
}

pub(super) struct Heartbeat {
    resume_epoch: u64,
    timer: Timer,
}

impl Heartbeat {
    pub(super) fn new(activity: WsActivity) -> Self {
        let mut heartbeat = Self {
            resume_epoch: activity.resume_epoch,
            timer: Timer::Idle,
        };
        heartbeat.update(activity);
        heartbeat
    }

    pub(super) fn update(&mut self, activity: WsActivity) {
        if !activity.maintain() {
            self.timer = Timer::Idle;
        } else if activity.resume_epoch != self.resume_epoch {
            self.timer = Timer::ProbeNow;
        } else if matches!(self.timer, Timer::Idle) {
            self.timer = Timer::PingAt(Instant::now() + HEARTBEAT);
        }
        self.resume_epoch = activity.resume_epoch;
    }

    pub(super) fn is_probing(&self) -> bool {
        matches!(self.timer, Timer::ProbeNow | Timer::AwaitPong(_))
    }

    pub(super) fn pong(&mut self, payload: &[u8]) {
        if matches!(self.timer, Timer::AwaitPong(_)) && payload == self.resume_epoch.to_be_bytes() {
            self.timer = Timer::PingAt(Instant::now() + HEARTBEAT);
        }
    }

    // Only mutate deadlines after the await: the socket reader can win select!
    // repeatedly without delaying periodic pings or extending a resume probe.
    pub(super) async fn tick(&mut self) -> HeartbeatAction {
        match self.timer {
            Timer::Idle => pending().await,
            Timer::PingAt(at) => {
                sleep_until(at).await;
                self.timer = Timer::PingAt(Instant::now() + HEARTBEAT);
                HeartbeatAction::Ping(Vec::new())
            }
            Timer::ProbeNow => {
                self.timer = Timer::AwaitPong(Instant::now() + PROBE_TIMEOUT);
                HeartbeatAction::Ping(self.resume_epoch.to_be_bytes().to_vec())
            }
            Timer::AwaitPong(at) => {
                sleep_until(at).await;
                HeartbeatAction::ProbeExpired
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use tokio::time::advance;

    #[tokio::test(start_paused = true)]
    async fn foreground_interrupts_backoff_even_with_an_ongoing_chat() {
        let policy = WsActivityPolicy::new(false);
        policy.update(|state| state.chats += 1);
        let mut activity = policy.0.subscribe();
        let (_endpoint, mut endpoint) = watch::channel(());
        let mut delay = Duration::from_secs(60);
        let started = Instant::now();
        let ((), ()) = tokio::join!(
            wait_retry(&mut endpoint, &mut activity, &mut delay),
            async {
                tokio::task::yield_now().await;
                policy.set_foreground(true);
            }
        );
        assert_eq!(started, Instant::now());
        assert_eq!(delay, Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_parks_the_timer_and_resume_requires_its_own_pong() {
        let mut state = WsActivity {
            foreground: false,
            chats: 1,
            resume_epoch: 0,
        };
        let mut heartbeat = Heartbeat::new(state);
        advance(HEARTBEAT - Duration::from_millis(1)).await;
        assert!(heartbeat.tick().now_or_never().is_none());
        advance(Duration::from_millis(1)).await;
        assert!(
            matches!(heartbeat.tick().await, HeartbeatAction::Ping(payload) if payload.is_empty())
        );

        state.chats = 0;
        heartbeat.update(state);
        advance(Duration::from_secs(600)).await;
        assert!(heartbeat.tick().now_or_never().is_none());

        state.set_foreground(true);
        heartbeat.update(state);
        let HeartbeatAction::Ping(first) = heartbeat.tick().await else {
            panic!("missing resume probe")
        };
        state.set_foreground(false);
        heartbeat.update(state);
        advance(PROBE_TIMEOUT).await;
        assert!(heartbeat.tick().now_or_never().is_none());

        state.set_foreground(true);
        heartbeat.update(state);
        let HeartbeatAction::Ping(second) = heartbeat.tick().await else {
            panic!("missing second probe")
        };
        assert_ne!(first, second);
        heartbeat.pong(&first);
        advance(PROBE_TIMEOUT).await;
        assert!(matches!(
            heartbeat.tick().await,
            HeartbeatAction::ProbeExpired
        ));
    }
}
