use std::time::Duration;

pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 2000,
        }
    }
}

pub async fn retry_on_db_locked<F, Fut, T>(
    config: &RetryConfig,
    mut operation: F,
    operation_name: &str,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut delay = config.base_delay_ms;
    let mut last_error = String::new();

    for attempt in 0..config.max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    println!(
                        "[Retry] {} succeeded on attempt {}",
                        operation_name, attempt + 1
                    );
                }
                return Ok(result);
            }
            Err(e) => {
                last_error = e.clone();

                if e.contains("database is locked") && attempt < config.max_retries - 1 {
                    println!(
                        "[Retry] {} failed with database locked, retrying in {}ms (attempt {}/{})",
                        operation_name, delay, attempt + 1, config.max_retries
                    );

                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    delay = (delay * 2).min(config.max_delay_ms);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(format!(
        "{} failed after {} retries: {}",
        operation_name, config.max_retries, last_error
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 2000);
    }

    #[tokio::test]
    async fn test_retry_success_on_first_attempt() {
        let config = RetryConfig::default();
        
        let result = retry_on_db_locked(&config, || async {
            Ok("success")
        }, "test_operation").await;
        
        assert_eq!(result.unwrap(), "success");
    }

    #[tokio::test]
    async fn test_retry_success_on_second_attempt() {
        let config = RetryConfig::default();
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        
        let result = retry_on_db_locked(&config, || {
            let attempt = attempt_clone.clone();
            async move {
                let count = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    Err("database is locked".to_string())
                } else {
                    Ok("success")
                }
            }
        }, "test_operation").await;
        
        assert_eq!(result.unwrap(), "success");
        assert_eq!(attempt.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_fails_after_max_attempts() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay_ms: 10,
            max_delay_ms: 100,
        };
        
        let result = retry_on_db_locked(&config, || async {
            Err("database is locked".to_string())
        }, "test_operation").await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed after 2 retries"));
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_immediate_failure() {
        let config = RetryConfig::default();
        
        let result = retry_on_db_locked(&config, || async {
            Err("not found".to_string())
        }, "test_operation").await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}