//! Build root to warm instance. Concurrent first callers share one boot,
//! which runs in a detached task so a caller that disconnects mid-boot can
//! never drop a half-spawned child; dead instances are replaced on the next
//! call; an optional reaper shuts down instances idle for too long.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

use crate::instance::{Instance, Phase, SbtConfig, ShutdownReport, StartError};
use crate::protocol::{InstancePhase, InstanceStatus};
use crate::routing::BuildRoot;

pub type StartOutcome = Result<Arc<Instance>, Arc<StartError>>;

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub sbt: SbtConfig,
    /// Instances idle this long are shut down by the reaper; `None` keeps
    /// them warm until an explicit shutdown.
    pub idle_timeout: Option<Duration>,
}

/// One build root's entry. `Starting` holds a channel every waiter shares;
/// the boot task turns it into `Ready` or removes it.
enum Slot {
    Starting(watch::Receiver<Option<StartOutcome>>),
    Ready(Arc<Instance>),
}

enum Claim {
    Ready(Arc<Instance>),
    Wait(watch::Receiver<Option<StartOutcome>>),
    Boot {
        report: watch::Sender<Option<StartOutcome>>,
        wait: watch::Receiver<Option<StartOutcome>>,
    },
}

pub struct Registry {
    slots: Mutex<HashMap<BuildRoot, Slot>>,
    config: RegistryConfig,
}

impl Registry {
    #[must_use]
    pub fn new(config: RegistryConfig) -> Arc<Self> {
        Arc::new(Self {
            slots: Mutex::new(HashMap::new()),
            config,
        })
    }

    #[must_use]
    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    /// The warm instance for `root`, booting one if none is alive. Callers
    /// arriving during a boot wait for that same boot and share its result.
    ///
    /// # Errors
    ///
    /// The boot failed; the error is shared by every waiter and the slot is
    /// cleared so a later call tries again.
    pub async fn get_or_start(self: &Arc<Self>, root: BuildRoot) -> StartOutcome {
        let wait = match self.claim(&root) {
            Claim::Ready(instance) => return Ok(instance),
            Claim::Wait(wait) => wait,
            Claim::Boot { report, wait } => {
                self.spawn_boot(root, report);
                wait
            }
        };
        await_outcome(wait).await
    }

    fn claim(&self, root: &BuildRoot) -> Claim {
        let mut slots = self.lock();
        match slots.get(root) {
            Some(Slot::Ready(instance)) if !instance.phase().is_dead() => {
                Claim::Ready(Arc::clone(instance))
            }
            Some(Slot::Starting(wait)) => Claim::Wait(wait.clone()),
            Some(Slot::Ready(_)) | None => {
                let (report, wait) = watch::channel(None);
                slots.insert(root.clone(), Slot::Starting(wait.clone()));
                Claim::Boot { report, wait }
            }
        }
    }

    fn spawn_boot(self: &Arc<Self>, root: BuildRoot, report: watch::Sender<Option<StartOutcome>>) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = Instance::start(&registry.config.sbt, root.clone())
                .await
                .map(Arc::new)
                .map_err(Arc::new);
            registry.settle(&root, &outcome);
            report.send_replace(Some(outcome));
        });
    }

    fn settle(&self, root: &BuildRoot, outcome: &StartOutcome) {
        let mut slots = self.lock();
        match outcome {
            Ok(instance) => {
                slots.insert(root.clone(), Slot::Ready(Arc::clone(instance)));
            }
            Err(error) => {
                tracing::warn!(%root, %error, "sbt failed to start");
                slots.remove(root);
            }
        }
    }

    /// Shut down `root`'s instance, waiting for a boot in progress first.
    /// `None` when there is no instance for `root`.
    pub async fn shutdown(&self, root: &BuildRoot) -> Option<ShutdownReport> {
        let instance = self.take(root).await?;
        Some(instance.shutdown().await)
    }

    async fn take(&self, root: &BuildRoot) -> Option<Arc<Instance>> {
        let slot = self.lock().remove(root)?;
        match slot {
            Slot::Ready(instance) => Some(instance),
            Slot::Starting(wait) => {
                let instance = await_outcome(wait).await.ok()?;
                self.lock().remove(root);
                Some(instance)
            }
        }
    }

    /// Shut down every instance, including ones still booting.
    pub async fn shutdown_all(self: &Arc<Self>) -> Vec<(BuildRoot, ShutdownReport)> {
        let mut reports = Vec::new();
        loop {
            let roots: Vec<BuildRoot> = self.lock().keys().cloned().collect();
            if roots.is_empty() {
                return reports;
            }
            let mut set = JoinSet::new();
            for root in roots {
                let registry = Arc::clone(self);
                set.spawn(async move {
                    let report = registry.shutdown(&root).await;
                    (root, report)
                });
            }
            while let Some(joined) = set.join_next().await {
                if let Ok((root, Some(report))) = joined {
                    reports.push((root, report));
                }
            }
        }
    }

    /// What `GET /status` reports. Dead instances are omitted; they are
    /// replaced on the next call for their build root.
    #[must_use]
    pub fn status(&self) -> Vec<InstanceStatus> {
        let now = Instant::now();
        let mut entries: Vec<InstanceStatus> = self
            .lock()
            .iter()
            .filter_map(|(root, slot)| status_of(root, slot, now))
            .collect();
        entries.sort_by(|a, b| a.build_root.cmp(&b.build_root));
        entries
    }

    /// Start the reaper when an idle timeout is configured. It also evicts
    /// dead instances it comes across.
    #[must_use]
    pub fn spawn_reaper(self: &Arc<Self>, interval: Duration) -> Option<JoinHandle<()>> {
        let timeout = self.config.idle_timeout?;
        let registry = Arc::clone(self);
        Some(tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                registry.reap_once(timeout).await;
            }
        }))
    }

    async fn reap_once(self: &Arc<Self>, timeout: Duration) {
        let now = Instant::now();
        let due: Vec<BuildRoot> = self
            .lock()
            .iter()
            .filter_map(|(root, slot)| match slot {
                Slot::Ready(instance) => match instance.phase() {
                    Phase::Idle { since } if now.saturating_duration_since(since) >= timeout => {
                        Some(root.clone())
                    }
                    Phase::Dead(_) => Some(root.clone()),
                    Phase::Idle { .. } | Phase::Booting | Phase::Running { .. } => None,
                },
                Slot::Starting(_) => None,
            })
            .collect();
        for root in due {
            tracing::info!(%root, "reaping an idle sbt instance");
            self.shutdown(&root).await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<BuildRoot, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn await_outcome(mut wait: watch::Receiver<Option<StartOutcome>>) -> StartOutcome {
    match wait.wait_for(Option::is_some).await {
        Ok(outcome) => outcome
            .clone()
            .unwrap_or(Err(Arc::new(StartError::Vanished))),
        Err(_) => Err(Arc::new(StartError::Vanished)),
    }
}

fn status_of(root: &BuildRoot, slot: &Slot, now: Instant) -> Option<InstanceStatus> {
    let build_root = root.as_path().to_path_buf();
    match slot {
        Slot::Starting(_) => Some(InstanceStatus {
            build_root,
            pid: None,
            phase: InstancePhase::Booting,
            idle_secs: None,
            uptime_secs: 0,
        }),
        Slot::Ready(instance) => {
            let (phase, idle_secs) = match instance.phase() {
                Phase::Booting => (InstancePhase::Booting, None),
                Phase::Idle { since } => (
                    InstancePhase::Idle,
                    Some(now.saturating_duration_since(since).as_secs()),
                ),
                Phase::Running { .. } => (InstancePhase::Running, None),
                Phase::Dead(_) => return None,
            };
            Some(InstanceStatus {
                build_root,
                pid: Some(instance.pid().get()),
                phase,
                idle_secs,
                uptime_secs: now
                    .saturating_duration_since(instance.started_at())
                    .as_secs(),
            })
        }
    }
}
