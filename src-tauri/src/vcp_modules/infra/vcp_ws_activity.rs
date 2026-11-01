use super::lifecycle_state::LifecycleState;
use futures_util::{FutureExt, Sink, SinkExt, Stream, StreamExt};
use std::future::pending;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::watch;
use tokio::time::{sleep, sleep_until, Instant};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

const HEARTBEAT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(super) struct WsActivity {
    foreground: bool,
    chats: usize,
    pub(super) probe_epoch: u64,
}

impl WsActivity {
    pub(super) fn maintain(self) -> bool {
        self.foreground || self.chats != 0
    }

    fn set_foreground(&mut self, foreground: bool) {
        if foreground && !self.foreground {
            self.probe_epoch = self.probe_epoch.wrapping_add(1);
        }
        self.foreground = foreground;
    }
}

#[derive(Clone)]
pub(super) struct WsActivityPolicy(watch::Sender<WsActivity>);

impl WsActivityPolicy {
    pub(super) fn acquire_chat(&self) -> ChatActivityGuard {
        self.update(|state| state.chats += 1);
        ChatActivityGuard(self.clone())
    }

    pub(super) fn new(foreground: bool) -> Self {
        let (sender, _) = watch::channel(WsActivity {
            foreground,
            chats: 0,
            probe_epoch: 0,
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

    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub(super) fn network_available(&self) {
        self.update(|state| {
            if state.maintain() {
                state.probe_epoch = state.probe_epoch.wrapping_add(1);
            }
        });
    }

    fn update(&self, change: impl FnOnce(&mut WsActivity)) {
        // Keep the full activity count. Wake listeners only when demand changes
        // or a foreground/network event requests a probe.
        self.0.send_if_modified(|state| {
            let previous = *state;
            change(state);
            state.maintain() != previous.maintain() || state.probe_epoch != previous.probe_epoch
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
        app.state::<LifecycleState>().ws_activity.acquire_chat()
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

pub(super) struct Retry {
    delay: Duration,
    immediate: bool,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(1),
            immediate: true,
        }
    }
}

impl Retry {
    /// One immediate replacement per outage. A new handshake alone must
    /// not rearm it when a peer repeatedly accepts and immediately closes.
    pub(super) fn disconnected(&mut self, healthy: bool) -> bool {
        if healthy {
            *self = Self::default();
        }
        std::mem::take(&mut self.immediate)
    }

    pub(super) async fn wait<T>(
        &mut self,
        endpoint: &mut watch::Receiver<T>,
        activity: &mut watch::Receiver<WsActivity>,
    ) {
        if activity.borrow().maintain() {
            tokio::select! {
                biased;
                _ = endpoint.changed() => {},
                _ = activity.changed() => {},
                _ = sleep(self.delay) => {
                    self.delay = (self.delay * 2).min(Duration::from_secs(60));
                    return;
                }
            }
        }
        *self = Self::default();
    }
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
    probe_epoch: u64,
    nonce: u64,
    timer: Timer,
    confirmed: bool,
    // If descheduled, a matching Pong may be behind already ready application
    // frames. Drain ready input without waiting for more bytes. Limit the time
    // spent draining so unrelated traffic cannot mask loss.
    drain_until: Option<Instant>,
}

pub(super) enum SocketEvent<T> {
    Timer(HeartbeatAction),
    Message(Option<T>),
}

impl Heartbeat {
    pub(super) fn new(activity: WsActivity) -> Self {
        let mut heartbeat = Self {
            probe_epoch: activity.probe_epoch,
            nonce: 0,
            timer: Timer::Idle,
            confirmed: false,
            drain_until: None,
        };
        heartbeat.update(activity);
        heartbeat
    }

    pub(super) fn update(&mut self, activity: WsActivity) -> bool {
        let probe = activity.maintain() && activity.probe_epoch != self.probe_epoch;
        if !activity.maintain() {
            self.timer = Timer::Idle;
        } else if probe {
            self.timer = Timer::ProbeNow;
        } else if matches!(self.timer, Timer::Idle) {
            self.timer = Timer::PingAt(Instant::now() + HEARTBEAT);
        }
        self.probe_epoch = activity.probe_epoch;
        probe
    }

    pub(super) fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub(super) async fn send_ping(
        &mut self,
        writer: &mut (impl Sink<Message, Error = WebSocketError> + Unpin),
        payload: Vec<u8>,
    ) -> Result<(), String> {
        tokio::time::timeout(PROBE_TIMEOUT, writer.send(Message::Ping(payload.into())))
            .await
            .map_err(|_| "WebSocket ping timed out".to_string())?
            .map_err(|error| error.to_string())?;
        // Write backpressure has its own deadline. Start the peer's full
        // response interval only after our Ping has actually been flushed.
        self.timer = Timer::AwaitPong(Instant::now() + PROBE_TIMEOUT);
        Ok(())
    }

    pub(super) fn pong(&mut self, payload: &[u8]) {
        if matches!(self.timer, Timer::AwaitPong(_)) && payload == self.nonce.to_be_bytes() {
            self.timer = Timer::PingAt(Instant::now() + HEARTBEAT);
            self.confirmed = true;
        }
    }

    pub(super) async fn next<S: Stream + Unpin>(&mut self, socket: &mut S) -> SocketEvent<S::Item> {
        let action = tokio::select! {
            biased;
            action = self.tick() => action,
            message = socket.next() => return SocketEvent::Message(message),
        };
        if matches!(action, HeartbeatAction::ProbeExpired)
            && self
                .drain_until
                .is_none_or(|deadline| Instant::now() < deadline)
        {
            if let Some(ready) = socket.next().now_or_never() {
                self.drain_until
                    .get_or_insert_with(|| Instant::now() + PROBE_TIMEOUT);
                return SocketEvent::Message(ready);
            }
        }
        SocketEvent::Timer(action)
    }

    // Only mutate deadlines after the await: the socket reader can win select!
    // repeatedly without delaying periodic pings or extending a resume probe.
    pub(super) async fn tick(&mut self) -> HeartbeatAction {
        match self.timer {
            Timer::Idle => pending().await,
            Timer::PingAt(at) => {
                sleep_until(at).await;
            }
            Timer::ProbeNow => {}
            Timer::AwaitPong(at) => {
                sleep_until(at).await;
                return HeartbeatAction::ProbeExpired;
            }
        }
        self.nonce = self.nonce.wrapping_add(1);
        self.timer = Timer::AwaitPong(Instant::now() + PROBE_TIMEOUT);
        self.drain_until = None;
        HeartbeatAction::Ping(self.nonce.to_be_bytes().to_vec())
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
        let mut retry = Retry {
            delay: Duration::from_secs(60),
            immediate: false,
        };
        let started = Instant::now();
        let ((), ()) = tokio::join!(retry.wait(&mut endpoint, &mut activity), async {
            tokio::task::yield_now().await;
            policy.set_foreground(true);
        });
        assert_eq!(started, Instant::now());
        assert_eq!(retry.delay, Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_parks_the_timer_and_resume_requires_its_own_pong() {
        let mut state = WsActivity {
            foreground: false,
            chats: 1,
            probe_epoch: 0,
        };
        let mut heartbeat = Heartbeat::new(state);
        advance(HEARTBEAT - Duration::from_millis(1)).await;
        assert!(heartbeat.tick().now_or_never().is_none());
        advance(Duration::from_millis(1)).await;
        assert!(
            matches!(heartbeat.tick().await, HeartbeatAction::Ping(payload) if !payload.is_empty())
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

    #[tokio::test(start_paused = true)]
    async fn background_chat_checks_regular_pongs_and_drains_a_ready_reply_after_sleep() {
        let policy = WsActivityPolicy::new(false);
        let _chat = policy.acquire_chat();
        let mut activity = policy.0.subscribe();
        let mut heartbeat = Heartbeat::new(*activity.borrow_and_update());
        advance(HEARTBEAT).await;
        let HeartbeatAction::Ping(first) = heartbeat.tick().await else {
            panic!("missing periodic ping")
        };
        advance(PROBE_TIMEOUT + Duration::from_secs(8)).await;
        // The peer's data and matching Pong were queued while we could not run.
        let mut ready =
            futures_util::stream::iter(std::iter::repeat_n(vec![0], 80).chain([first.clone()]));
        for _ in 0..80 {
            assert!(
                matches!(heartbeat.next(&mut ready).await, SocketEvent::Message(Some(value)) if value == [0])
            );
        }
        let SocketEvent::Message(Some(pong)) = heartbeat.next(&mut ready).await else {
            panic!("dropped queued pong")
        };
        heartbeat.pong(&pong);
        assert!(heartbeat.confirmed());
        advance(HEARTBEAT).await;
        let HeartbeatAction::Ping(second) = heartbeat.tick().await else {
            panic!("missing next ping")
        };
        assert_ne!(first, second);
        heartbeat.pong(&first);
        advance(PROBE_TIMEOUT).await;
        let mut stalled = futures_util::stream::pending::<Vec<u8>>();
        assert!(matches!(
            heartbeat.next(&mut stalled).await,
            SocketEvent::Timer(HeartbeatAction::ProbeExpired)
        ));
        policy.network_available();
        heartbeat.update(*activity.borrow_and_update());
        let HeartbeatAction::Ping(third) = heartbeat.tick().await else {
            panic!("missing new path probe")
        };
        assert_ne!(third, second);
        heartbeat.pong(&second);
        assert!(heartbeat.tick().now_or_never().is_none());
        advance(PROBE_TIMEOUT).await;
        let mut noise = futures_util::stream::repeat(vec![0]);
        assert!(matches!(
            heartbeat.next(&mut noise).await,
            SocketEvent::Message(Some(_))
        ));
        advance(PROBE_TIMEOUT).await;
        assert!(matches!(
            heartbeat.next(&mut noise).await,
            SocketEvent::Timer(HeartbeatAction::ProbeExpired)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_short_connections_back_off_until_a_confirmed_socket_or_network_hint() {
        let policy = WsActivityPolicy::new(true);
        let mut activity = policy.0.subscribe();
        let (_send, mut endpoint) = watch::channel(());
        let mut retry = Retry::default();
        assert!(retry.disconnected(true));
        for seconds in [1, 2, 4, 8, 16, 32, 60, 60] {
            assert!(!retry.disconnected(false));
            let start = Instant::now();
            retry.wait(&mut endpoint, &mut activity).await;
            assert_eq!(start.elapsed(), Duration::from_secs(seconds));
        }
        let start = Instant::now();
        let ((), ()) = tokio::join!(retry.wait(&mut endpoint, &mut activity), async {
            tokio::task::yield_now().await;
            policy.network_available();
        });
        assert_eq!(start, Instant::now());
        assert_eq!(retry.delay, Duration::from_secs(1));
        assert!(retry.disconnected(false));
        assert!(!retry.disconnected(false));
        assert!(retry.disconnected(true));
        policy.set_foreground(false);
        activity.borrow_and_update();
        policy.network_available();
        assert!(!activity.has_changed().unwrap());
    }
}
