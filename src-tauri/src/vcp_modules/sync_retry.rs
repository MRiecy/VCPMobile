use std::time::Duration;
use tokio::time::sleep;
use rand::Rng;

pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 200,
            max_delay_ms: 5000,
            jitter_factor: 0.1,
        }
    }
}

impl RetryPolicy {
    pub fn default_clone(&self) -> Self {
        Self {
            max_retries: self.max_retries,
            base_delay_ms: self.base_delay_ms,
            max_delay_ms: self.max_delay_ms,
            jitter_factor: self.jitter_factor,
        }
    }

    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let exponential_delay = self.base_delay_ms * 2u64.pow(attempt);
        let capped_delay = exponential_delay.min(self.max_delay_ms);
        
        // Add jitter
        let jitter_range = (capped_delay as f64 * self.jitter_factor) as u64;
        let jitter = if jitter_range > 0 {
            rand::thread_rng().gen_range(0..=jitter_range)
        } else {
            0
        };
        
        Duration::from_millis(capped_delay + jitter)
    }
    
    pub async fn execute_with_retry<T, E, F, Fut>(
        &self,
        operation: F,
        is_retryable: impl Fn(&E) -> bool,
    ) -> Result<T, E>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        let mut last_error = None;
        
        for attempt in 0..=self.max_retries {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) || attempt == self.max_retries {
                        return Err(e);
                    }
                    
                    let delay = self.calculate_delay(attempt);
                    println!("[SyncRetry] Attempt {} failed, retrying in {:?}", attempt + 1, delay);
                    sleep(delay).await;
                    last_error = Some(e);
                }
            }
        }
        
        Err(last_error.expect("At least one error occurred"))
    }
}

pub fn is_network_retryable(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("timeout") || 
    e.contains("connection reset") ||
    e.contains("connection refused") ||
    e.contains("502") || e.contains("503") || e.contains("504") || e.contains("429")
}
