use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::Result;

pub const USER_AGENT: &str = concat!(
    "tmaps/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// # Errors
///
/// [`Error::Network`](crate::Error::Network) if the TLS backend cannot start.
pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// Spaces calls at least `interval` apart, in the order the callers arrive.
pub(crate) struct Throttle {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl Throttle {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
        }
    }

    pub(crate) async fn wait(&self) {
        let mut last = self.last.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + self.interval).await;
        }
        *last = Some(Instant::now());
    }
}

pub(crate) fn body_snippet(body: &[u8]) -> String {
    const MAX_CHARS: usize = 200;
    String::from_utf8_lossy(body)
        .chars()
        .take(MAX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn throttle_spaces_calls_by_interval() {
        let throttle = Throttle::new(Duration::from_secs(1));
        let start = Instant::now();
        throttle.wait().await;
        throttle.wait().await;
        throttle.wait().await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }
}
