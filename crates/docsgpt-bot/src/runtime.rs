//! Concurrency helpers: one turn at a time per scope, stoppable turns, and
//! graceful shutdown that lets turns in progress finish.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Serializes work per key (usually [`crate::Scope::key`]) so two messages in
/// the same chat don't run their turns interleaved.
#[derive(Default)]
pub struct ScopeLocks {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Entries kept before unused locks are dropped.
const PRUNE_AT: usize = 5000;

impl ScopeLocks {
    /// Wait for `key`'s lock. Hold the guard for the whole turn.
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        self.entry(key).lock_owned().await
    }

    /// The lock if it is free right now, else `None` (e.g. to tell the user to wait).
    pub fn try_lock(&self, key: &str) -> Option<OwnedMutexGuard<()>> {
        self.entry(key).try_lock_owned().ok()
    }

    fn entry(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.locks.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() >= PRUNE_AT {
            map.retain(|_, m| Arc::strong_count(m) > 1);
        }
        map.entry(key.to_string()).or_default().clone()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.locks.lock().unwrap().len()
    }
}

/// Running turns that a user can stop, by a key the platform knows (a message
/// id, a draft id, a session id).
#[derive(Default, Clone)]
pub struct CancelRegistry {
    inner: Arc<Mutex<Registrations>>,
}

#[derive(Default)]
struct Registrations {
    next_id: u64,
    tokens: HashMap<String, (u64, CancellationToken)>,
}

/// A registered turn. Dropping it unregisters the key.
pub struct CancelGuard {
    key: String,
    id: u64,
    token: CancellationToken,
    registry: CancelRegistry,
}

impl CancelGuard {
    /// The token the turn should watch.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        let mut inner = self.registry.lock();
        // Only remove our own registration; the key may have been registered again.
        if inner.tokens.get(&self.key).is_some_and(|(id, _)| *id == self.id) {
            inner.tokens.remove(&self.key);
        }
    }
}

impl CancelRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registrations> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register a turn under `key`, replacing any earlier one with the same key.
    /// `parent` (e.g. the shutdown token) also cancels it.
    pub fn register(&self, key: impl Into<String>, parent: Option<&CancellationToken>) -> CancelGuard {
        self.insert(key, parent.map(CancellationToken::child_token).unwrap_or_default())
    }

    /// Register an existing token under `key` (e.g. a turn's own token, once the
    /// platform knows the id the user will press Stop on).
    pub fn insert(&self, key: impl Into<String>, token: CancellationToken) -> CancelGuard {
        let key = key.into();
        let mut inner = self.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        inner.tokens.insert(key.clone(), (id, token.clone()));
        drop(inner);
        CancelGuard {
            key,
            id,
            token,
            registry: self.clone(),
        }
    }

    /// Stop the turn registered under `key`. Returns false if none is running.
    pub fn cancel(&self, key: &str) -> bool {
        match self.lock().tokens.get(key) {
            Some((_, t)) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Number of registered turns.
    pub fn len(&self) -> usize {
        self.lock().tokens.len()
    }

    /// True when no turn is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Process lifetime: a root cancellation token, and a tracker for spawned work
/// so shutdown can wait for turns in progress.
#[derive(Clone, Default)]
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
}

impl Shutdown {
    /// A fresh, running lifetime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancelled when shutdown starts. Long-polling loops and servers watch it.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// True once shutdown has started.
    pub fn is_shutting_down(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Spawn work that shutdown waits for (one per incoming message or event).
    pub fn spawn<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tracker.spawn(fut)
    }

    /// Start shutting down.
    pub fn trigger(&self) {
        self.token.cancel();
    }

    /// Wait for SIGINT or SIGTERM (Ctrl-C on Windows), then start shutting down.
    pub async fn wait_for_signal(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
                _ = self.token.cancelled() => {}
            }
        }
        #[cfg(not(unix))]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = self.token.cancelled() => {}
        }
        tracing::info!("shutting down");
        self.trigger();
    }

    /// After [`Shutdown::trigger`], wait up to `grace` for spawned work. Work
    /// still running then is cancelled through the token and abandoned.
    /// Returns true if everything finished in time.
    pub async fn drain(&self, grace: Duration) -> bool {
        self.tracker.close();
        let done = tokio::time::timeout(grace, self.tracker.wait()).await.is_ok();
        if !done {
            tracing::warn!(
                remaining = self.tracker.len(),
                "shutdown grace period over; abandoning turns in progress"
            );
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn scope_lock_serializes_and_prunes() {
        let locks = Arc::new(ScopeLocks::default());
        let g = locks.lock("a").await;
        assert!(locks.try_lock("a").is_none());
        assert!(locks.try_lock("b").is_some());
        let l2 = locks.clone();
        let waiter = tokio::spawn(async move { l2.lock("a").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(g);
        drop(waiter.await.unwrap());

        for i in 0..PRUNE_AT + 10 {
            drop(locks.try_lock(&format!("k{i}")));
        }
        assert!(locks.len() < PRUNE_AT);
    }

    #[tokio::test]
    async fn cancel_registry() {
        let reg = CancelRegistry::default();
        assert!(!reg.cancel("x"));
        let g = reg.register("x", None);
        assert_eq!(reg.len(), 1);
        assert!(reg.cancel("x"));
        assert!(g.token().is_cancelled());
        drop(g);
        assert!(reg.is_empty());

        // Re-registering a key replaces it; dropping the old guard keeps the new one.
        let old = reg.register("k", None);
        let new = reg.register("k", None);
        drop(old);
        assert!(reg.cancel("k"));
        assert!(new.token().is_cancelled());
        drop(new);
        assert!(reg.is_empty());

        let parent = CancellationToken::new();
        let g = reg.register("p", Some(&parent));
        parent.cancel();
        assert!(g.token().is_cancelled());
    }

    #[tokio::test]
    async fn shutdown_drains_spawned_work() {
        let sd = Shutdown::new();
        let done = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let d = done.clone();
            sd.spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                d.fetch_add(1, Ordering::SeqCst);
            });
        }
        sd.trigger();
        assert!(sd.is_shutting_down());
        assert!(sd.drain(Duration::from_secs(5)).await);
        assert_eq!(done.load(Ordering::SeqCst), 3);

        let sd = Shutdown::new();
        sd.spawn(std::future::pending::<()>());
        sd.trigger();
        assert!(!sd.drain(Duration::from_millis(50)).await);
    }
}
