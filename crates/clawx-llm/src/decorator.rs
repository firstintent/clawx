use async_trait::async_trait;
use clawx_core::{Error, Message, Result, ToolDefinition};
use crate::provider::Provider;
use crate::stream::StreamEvent;
use tokio::sync::mpsc;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// RetryProvider — exponential backoff with jitter (IronClaw pattern)
// ---------------------------------------------------------------------------

pub struct RetryProvider {
    inner: Arc<dyn Provider>,
    max_retries: usize,
    base_delay_ms: u64,
    jitter_pct: f64,
}

impl RetryProvider {
    pub fn new(inner: Arc<dyn Provider>, max_retries: usize, base_delay_ms: u64, jitter_pct: f64) -> Self {
        Self { inner, max_retries, base_delay_ms, jitter_pct }
    }

    fn delay_for_attempt(&self, attempt: usize) -> std::time::Duration {
        let base = self.base_delay_ms as f64 * 2.0_f64.powi(attempt as i32);
        let jitter_range = base * self.jitter_pct;
        // Deterministic-ish jitter using attempt number (avoid rand dependency)
        let jitter = jitter_range * ((attempt as f64 * 0.618).fract() * 2.0 - 1.0);
        let ms = (base + jitter).max(100.0) as u64;
        std::time::Duration::from_millis(ms)
    }
}

#[async_trait]
impl Provider for RetryProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Message> {
        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            match self.inner.complete(messages, tools).await {
                Ok(msg) => return Ok(msg),
                Err(e) if e.is_transient() && attempt < self.max_retries => {
                    let delay = self.delay_for_attempt(attempt);
                    warn!(attempt, ?delay, error = %e, "retrying LLM call");
                    tokio::time::sleep(delay).await;
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Other("retry exhausted".into())))
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Message> {
        // For streaming, retry only on initial connection failure.
        // Once streaming starts, we don't retry mid-stream.
        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            match self.inner.stream(messages, tools, tx.clone()).await {
                Ok(msg) => return Ok(msg),
                Err(e) if e.is_transient() && attempt < self.max_retries => {
                    let delay = self.delay_for_attempt(attempt);
                    warn!(attempt, ?delay, error = %e, "retrying LLM stream");
                    tokio::time::sleep(delay).await;
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Other("retry exhausted".into())))
    }

    fn name(&self) -> &str { self.inner.name() }
    fn model(&self) -> &str { self.inner.model() }
}

// ---------------------------------------------------------------------------
// CircuitBreakerProvider — Closed → Open → HalfOpen (IronClaw pattern)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreakerProvider {
    inner: Arc<dyn Provider>,
    failure_count: AtomicUsize,
    threshold: usize,
    recovery_secs: u64,
    /// Timestamp (epoch secs) when circuit opened.
    opened_at: AtomicU64,
}

impl CircuitBreakerProvider {
    pub fn new(inner: Arc<dyn Provider>, threshold: usize, recovery_secs: u64) -> Self {
        Self {
            inner,
            failure_count: AtomicUsize::new(0),
            threshold,
            recovery_secs,
            opened_at: AtomicU64::new(0),
        }
    }

    fn state(&self) -> CircuitState {
        let failures = self.failure_count.load(Ordering::Relaxed);
        if failures < self.threshold {
            return CircuitState::Closed;
        }
        let opened = self.opened_at.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now - opened >= self.recovery_secs {
            CircuitState::HalfOpen
        } else {
            CircuitState::Open
        }
    }

    fn record_success(&self) {
        self.failure_count.store(0, Ordering::Relaxed);
        self.opened_at.store(0, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        let prev = self.failure_count.fetch_add(1, Ordering::Relaxed);
        if prev + 1 >= self.threshold {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.opened_at.store(now, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl Provider for CircuitBreakerProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Message> {
        if self.state() == CircuitState::Open {
            return Err(Error::Provider("circuit breaker open".into()));
        }
        match self.inner.complete(messages, tools).await {
            Ok(msg) => {
                self.record_success();
                Ok(msg)
            }
            Err(e) => {
                self.record_failure();
                Err(e)
            }
        }
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Message> {
        if self.state() == CircuitState::Open {
            return Err(Error::Provider("circuit breaker open".into()));
        }
        match self.inner.stream(messages, tools, tx).await {
            Ok(msg) => {
                self.record_success();
                Ok(msg)
            }
            Err(e) => {
                self.record_failure();
                Err(e)
            }
        }
    }

    fn name(&self) -> &str { self.inner.name() }
    fn model(&self) -> &str { self.inner.model() }
}

// ---------------------------------------------------------------------------
// FailoverProvider — switch to backup after N failures (IronClaw pattern)
// ---------------------------------------------------------------------------

pub struct FailoverProvider {
    primary: Arc<dyn Provider>,
    fallback: Arc<dyn Provider>,
    failure_count: AtomicUsize,
    threshold: usize,
    cooldown_secs: u64,
    failed_at: AtomicU64,
}

impl FailoverProvider {
    pub fn new(
        primary: Arc<dyn Provider>,
        fallback: Arc<dyn Provider>,
        threshold: usize,
        cooldown_secs: u64,
    ) -> Self {
        Self {
            primary,
            fallback,
            failure_count: AtomicUsize::new(0),
            threshold,
            cooldown_secs,
            failed_at: AtomicU64::new(0),
        }
    }

    fn active(&self) -> &Arc<dyn Provider> {
        let failures = self.failure_count.load(Ordering::Relaxed);
        if failures < self.threshold {
            return &self.primary;
        }
        let failed_at = self.failed_at.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now - failed_at >= self.cooldown_secs {
            // Reset and try primary again
            self.failure_count.store(0, Ordering::Relaxed);
            info!("failover cooldown expired, switching back to primary");
            &self.primary
        } else {
            &self.fallback
        }
    }

    fn record_failure(&self) {
        let prev = self.failure_count.fetch_add(1, Ordering::Relaxed);
        if prev + 1 >= self.threshold {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.failed_at.store(now, Ordering::Relaxed);
            warn!("failover threshold reached, switching to fallback provider");
        }
    }
}

#[async_trait]
impl Provider for FailoverProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Message> {
        match self.active().complete(messages, tools).await {
            Ok(msg) => Ok(msg),
            Err(_e) => {
                self.record_failure();
                // Try fallback immediately on failure
                self.fallback.complete(messages, tools).await
            }
        }
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Message> {
        match self.active().stream(messages, tools, tx.clone()).await {
            Ok(msg) => Ok(msg),
            Err(_e) => {
                self.record_failure();
                self.fallback.stream(messages, tools, tx).await
            }
        }
    }

    fn name(&self) -> &str { self.active().name() }
    fn model(&self) -> &str { self.active().model() }
}
