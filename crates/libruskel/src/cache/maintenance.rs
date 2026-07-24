//! Coalesced cache maintenance and deterministic retention policy.

use std::{
    fmt, io,
    path::Path,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{
    inventory::{CacheInventory, WorkspaceInventory},
    layout::{self, CacheLayout, LAST_USE},
    owner::HIGH_WATER_BYTES,
    report::CacheIssue,
};
use crate::error::{Result, RuskelError};

/// Routine-maintenance pending bit.
const REASON_ROUTINE: u8 = 1;
/// Urgent low-space pending bit.
const REASON_URGENT: u8 = 2;
/// Routine maintenance interval.
const ROUTINE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Retention period for an inactive noncurrent toolchain.
const TOOLCHAIN_RETENTION: Duration = Duration::from_secs(60 * 60);
/// Retention period for a workspace entry.
const WORKSPACE_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// Minimum free space before maintenance becomes urgent.
const LOW_SPACE_BYTES: u64 = 1_000_000_000;
/// Budget target after high-water eviction starts.
const EVICTION_TARGET_BYTES: u64 = 15_000_000_000;

/// Thread-safe provider for available filesystem space.
type SpaceProvider = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;

/// Runtime hooks that make time, space, and thresholds deterministic in tests.
#[derive(Clone)]
struct MaintenanceHooks {
    /// Current wall-clock provider.
    now: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    /// Available-space provider.
    available_space: Arc<SpaceProvider>,
    /// Interval between routine passes.
    routine_interval: Duration,
    /// Inactive toolchain retention.
    toolchain_retention: Duration,
    /// Workspace retention.
    workspace_retention: Duration,
    /// Soft high-water mark.
    high_water_bytes: u64,
    /// Usage target after eviction.
    eviction_target_bytes: u64,
    /// Low-space threshold.
    low_space_bytes: u64,
    /// Optional deterministic notification before a synchronous GC wait.
    gc_wait_observer: Option<Arc<Barrier>>,
}

impl fmt::Debug for MaintenanceHooks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaintenanceHooks")
            .field("routine_interval", &self.routine_interval)
            .field("toolchain_retention", &self.toolchain_retention)
            .field("workspace_retention", &self.workspace_retention)
            .field("high_water_bytes", &self.high_water_bytes)
            .field("eviction_target_bytes", &self.eviction_target_bytes)
            .field("low_space_bytes", &self.low_space_bytes)
            .field("has_gc_wait_observer", &self.gc_wait_observer.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for MaintenanceHooks {
    fn default() -> Self {
        Self {
            now: Arc::new(SystemTime::now),
            available_space: Arc::new(|path| fs4::available_space(path)),
            routine_interval: ROUTINE_INTERVAL,
            toolchain_retention: TOOLCHAIN_RETENTION,
            workspace_retention: WORKSPACE_RETENTION,
            high_water_bytes: HIGH_WATER_BYTES,
            eviction_target_bytes: EVICTION_TARGET_BYTES,
            low_space_bytes: LOW_SPACE_BYTES,
            gc_wait_observer: None,
        }
    }
}

/// Result summary for one internal maintenance request.
#[derive(Debug, Default)]
pub(super) struct MaintenanceResult {
    /// Whether this request performed a pass.
    pub(super) ran: bool,
    /// Whether this request waited for another completed pass.
    pub(super) waited: bool,
    /// Number of removed owned entries.
    pub(super) removed_entries: u64,
    /// Number of recognized bytes removed.
    pub(super) removed_bytes: u64,
    /// Best-effort issues that did not make the pass unsafe.
    pub(super) issues: Vec<CacheIssue>,
}

/// One bounded maintenance worker owned by a cache owner.
pub(super) struct MaintenanceWorker {
    /// Bounded wake sender, removed during shutdown.
    sender: Mutex<Option<SyncSender<()>>>,
    /// Coalesced reason bits.
    pending: Arc<AtomicU8>,
    /// Most recently observed current toolchain identity.
    current_toolchain: Arc<Mutex<Option<String>>>,
    /// Worker join handle.
    handle: Mutex<Option<JoinHandle<()>>>,
    /// Providers used for low-space classification.
    hooks: MaintenanceHooks,
}

impl fmt::Debug for MaintenanceWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaintenanceWorker")
            .field("pending", &self.pending.load(Ordering::Relaxed))
            .field("hooks", &self.hooks)
            .finish_non_exhaustive()
    }
}

impl MaintenanceWorker {
    /// Start one worker for a validated layout.
    pub(super) fn start(layout: CacheLayout) -> Result<Self> {
        Self::start_with_hooks(layout, MaintenanceHooks::default())
    }

    /// Start one worker with deterministic providers.
    fn start_with_hooks(layout: CacheLayout, hooks: MaintenanceHooks) -> Result<Self> {
        let (sender, receiver) = sync_channel(1);
        let pending = Arc::new(AtomicU8::new(0));
        let current_toolchain = Arc::new(Mutex::new(None));
        let worker_pending = Arc::clone(&pending);
        let worker_current = Arc::clone(&current_toolchain);
        let worker_hooks = hooks.clone();
        let handle = thread::Builder::new()
            .name("ruskel-cache-maintenance".to_string())
            .spawn(move || {
                worker_loop(
                    &layout,
                    &worker_hooks,
                    &worker_pending,
                    &worker_current,
                    &receiver,
                );
            })
            .map_err(|error| {
                RuskelError::Generate(format!(
                    "Failed to start the Ruskel cache maintenance worker: {error}"
                ))
            })?;

        Ok(Self {
            sender: Mutex::new(Some(sender)),
            pending,
            current_toolchain,
            handle: Mutex::new(Some(handle)),
            hooks,
        })
    }

    /// Return whether the cache filesystem is below the low-space threshold.
    pub(super) fn is_low_space(&self, root: &Path) -> bool {
        (self.hooks.available_space)(root)
            .is_ok_and(|available| available < self.hooks.low_space_bytes)
    }

    /// Submit one coalesced routine or urgent signal.
    pub(super) fn signal(&self, current_toolchain: &str, urgent: bool) {
        if let Ok(mut current) = self.current_toolchain.lock() {
            *current = Some(current_toolchain.to_string());
        }
        let reason = if urgent {
            REASON_URGENT
        } else {
            REASON_ROUTINE
        };
        self.pending.fetch_or(reason, Ordering::Release);

        if let Ok(sender) = self.sender.lock()
            && let Some(sender) = sender.as_ref()
        {
            match sender.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) | Err(TrySendError::Disconnected(())) => {}
            }
        }
    }

    /// Run forced synchronous recovery or wait for the equivalent active pass.
    pub(super) fn recover(
        &self,
        layout: &CacheLayout,
        current_toolchain: &str,
    ) -> Result<MaintenanceResult> {
        run_pass(
            layout,
            &self.hooks,
            Some(current_toolchain),
            PassMode::Synchronous,
        )
    }

    /// Close the wake channel, drain pending work, and join the worker.
    pub(super) fn shutdown(&self) {
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        if let Ok(mut handle) = self.handle.lock()
            && let Some(handle) = handle.take()
        {
            drop(handle.join());
        }
    }
}

/// Maintenance lock and schedule mode.
#[derive(Clone, Copy, Debug)]
enum PassMode {
    /// Do not wait for the GC lock and honor the routine interval.
    Routine,
    /// Wait for active GC, otherwise perform one pass without the interval limit.
    Synchronous,
}

/// Drain wake messages and execute coalesced passes.
fn worker_loop(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    pending: &AtomicU8,
    current_toolchain: &Mutex<Option<String>>,
    receiver: &Receiver<()>,
) {
    while receiver.recv().is_ok() {
        run_pending(layout, hooks, pending, current_toolchain);
    }
    run_pending(layout, hooks, pending, current_toolchain);
}

/// Run one pass for all currently pending reason bits.
fn run_pending(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    pending: &AtomicU8,
    current_toolchain: &Mutex<Option<String>>,
) {
    let reason = pending.swap(0, Ordering::AcqRel);
    if reason == 0 {
        return;
    }
    let current = current_toolchain
        .lock()
        .ok()
        .and_then(|value| value.clone());
    let mode = if reason & REASON_URGENT != 0 {
        PassMode::Synchronous
    } else {
        PassMode::Routine
    };
    drop(run_pass(layout, hooks, current.as_deref(), mode));
}

/// Acquire maintenance leases and run one ordered collection pass.
fn run_pass(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    current_toolchain: Option<&str>,
    mode: PassMode,
) -> Result<MaintenanceResult> {
    let _root = layout.lock_root_shared()?;
    let gc_path = layout.gc_lock();
    let gc_lease = match layout.try_lock_exclusive(&gc_path)? {
        Some(lease) => lease,
        None if matches!(mode, PassMode::Routine) => return Ok(MaintenanceResult::default()),
        None => {
            if let Some(observer) = &hooks.gc_wait_observer {
                observer.wait();
            }
            let waited_lease = layout.lock_exclusive(&gc_path)?;
            drop(waited_lease);
            return Ok(MaintenanceResult {
                waited: true,
                ..MaintenanceResult::default()
            });
        }
    };

    let now = unix_seconds((hooks.now)())?;
    if matches!(mode, PassMode::Routine) && !routine_due(layout, now, hooks.routine_interval) {
        drop(gc_lease);
        return Ok(MaintenanceResult::default());
    }

    let mut result = MaintenanceResult {
        ran: true,
        ..MaintenanceResult::default()
    };
    cleanup_trash(layout, &mut result)?;
    collect_old_toolchains(layout, hooks, current_toolchain, now, &mut result)?;
    collect_old_workspaces(layout, hooks, now, &mut result)?;
    enforce_budget(layout, hooks, now, &mut result)?;
    layout::write_timestamp(&layout.maintenance_stamp(), now)?;
    drop(gc_lease);
    Ok(result)
}

/// Return whether the routine interval has elapsed.
fn routine_due(layout: &CacheLayout, now: u64, interval: Duration) -> bool {
    match layout::read_timestamp(&layout.maintenance_stamp()) {
        Ok(Some(last)) if last <= now => now.saturating_sub(last) >= interval.as_secs(),
        Ok(Some(_)) | Ok(None) | Err(_) => true,
    }
}

/// Remove recognized trash left by interrupted deletion.
fn cleanup_trash(layout: &CacheLayout, result: &mut MaintenanceResult) -> Result<()> {
    let inventory = collect_inventory(layout, result)?;
    for trash in inventory.trash {
        if !trash.revalidate() {
            push_issue(
                result,
                CacheIssue::new(trash.path, "trash entry changed after inventory"),
            );
            continue;
        }
        remove_candidate(&trash.path, result);
    }
    Ok(())
}

/// Remove inactive noncurrent toolchain trees after the retention period.
fn collect_old_toolchains(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    current_toolchain: Option<&str>,
    now: u64,
    result: &mut MaintenanceResult,
) -> Result<()> {
    let inventory = collect_inventory(layout, result)?;
    for toolchain in inventory.toolchains {
        if current_toolchain == Some(toolchain.identity.as_str()) {
            continue;
        }
        let Some(last_use) = valid_past_timestamp(
            toolchain.last_use,
            &toolchain.path.join(LAST_USE),
            now,
            result,
        ) else {
            continue;
        };
        if now.saturating_sub(last_use) < hooks.toolchain_retention.as_secs() {
            continue;
        }
        let lock_path = layout.toolchain_lock(&toolchain.identity);
        let Some(lease) = layout.try_lock_exclusive(&lock_path)? else {
            continue;
        };
        if !toolchain.revalidate() {
            push_issue(
                result,
                CacheIssue::new(toolchain.path, "toolchain entry changed after inventory"),
            );
            drop(lease);
            continue;
        }
        let label = format!("{}.{}", toolchain.identity, toolchain.identity);
        match layout::move_to_trash(layout.root(), &toolchain.path, &label) {
            Ok(trash) => remove_candidate(&trash, result),
            Err(error) => push_issue(result, CacheIssue::new(toolchain.path, error.to_string())),
        }
        drop(lease);
    }
    Ok(())
}

/// Remove workspace entries after their retention period.
fn collect_old_workspaces(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    now: u64,
    result: &mut MaintenanceResult,
) -> Result<()> {
    let inventory = collect_inventory(layout, result)?;
    for toolchain in inventory.toolchains {
        for workspace in toolchain.workspaces {
            let Some(last_use) = valid_past_timestamp(
                workspace.last_use,
                &workspace.path.join(LAST_USE),
                now,
                result,
            ) else {
                continue;
            };
            if now.saturating_sub(last_use) < hooks.workspace_retention.as_secs() {
                continue;
            }
            let lock_path = layout.workspace_lock(&workspace.identity);
            let Some(lease) = layout.try_lock_exclusive(&lock_path)? else {
                continue;
            };
            if !workspace.revalidate() {
                push_issue(
                    result,
                    CacheIssue::new(workspace.path, "workspace entry changed after inventory"),
                );
                drop(lease);
                continue;
            }
            let label = format!("{}.{}", workspace.toolchain, workspace.identity);
            match layout::move_to_trash(layout.root(), &workspace.path, &label) {
                Ok(trash) => remove_candidate(&trash, result),
                Err(error) => {
                    push_issue(result, CacheIssue::new(workspace.path, error.to_string()))
                }
            }
            drop(lease);
        }
    }
    Ok(())
}

/// Evict oldest valid workspaces until usage reaches the low target.
fn enforce_budget(
    layout: &CacheLayout,
    hooks: &MaintenanceHooks,
    now: u64,
    result: &mut MaintenanceResult,
) -> Result<()> {
    let inventory = collect_inventory(layout, result)?;
    let mut candidates: Vec<(WorkspaceInventory, u64)> = inventory
        .toolchains
        .iter()
        .flat_map(|toolchain| toolchain.workspaces.iter())
        .filter_map(|workspace| {
            let last_use = valid_past_timestamp(
                workspace.last_use,
                &workspace.path.join(LAST_USE),
                now,
                result,
            )?;
            workspace.size_bytes.map(|_| (workspace.clone(), last_use))
        })
        .collect();
    let mut usage = inventory.recognized_usage();
    if usage <= hooks.high_water_bytes || candidates.len() <= 1 {
        return Ok(());
    }
    candidates.sort_by_key(|(_, last_use)| *last_use);
    let newest = candidates
        .iter()
        .max_by_key(|(_, last_use)| *last_use)
        .map(|(candidate, _)| candidate.path.clone());

    for (candidate, _) in candidates {
        if usage <= hooks.eviction_target_bytes || newest.as_ref() == Some(&candidate.path) {
            continue;
        }
        let lock_path = layout.workspace_lock(&candidate.identity);
        let Some(lease) = layout.try_lock_exclusive(&lock_path)? else {
            continue;
        };
        if !candidate.revalidate() {
            push_issue(
                result,
                CacheIssue::new(candidate.path, "workspace entry changed after inventory"),
            );
            drop(lease);
            continue;
        }
        let label = format!("{}.{}", candidate.toolchain, candidate.identity);
        match layout::move_to_trash(layout.root(), &candidate.path, &label) {
            Ok(trash) => {
                remove_candidate(&trash, result);
                usage = usage.saturating_sub(candidate.size_bytes.unwrap_or(0));
            }
            Err(error) => push_issue(result, CacheIssue::new(candidate.path, error.to_string())),
        }
        drop(lease);
    }
    Ok(())
}

/// Read an eligible timestamp that is not in the future.
fn valid_past_timestamp(
    value: Option<u64>,
    path: &Path,
    now: u64,
    result: &mut MaintenanceResult,
) -> Option<u64> {
    match value {
        Some(value) if value <= now => Some(value),
        Some(_) => {
            push_issue(
                result,
                CacheIssue::new(path, "last-use metadata is in the future"),
            );
            None
        }
        None => None,
    }
}

/// Collect one inventory and merge its issues without duplicates.
fn collect_inventory(
    layout: &CacheLayout,
    result: &mut MaintenanceResult,
) -> Result<CacheInventory> {
    let inventory = CacheInventory::collect(layout)?;
    for issue in &inventory.issues {
        push_issue(result, issue.clone());
    }
    Ok(inventory)
}

/// Append one issue only once per maintenance pass.
fn push_issue(result: &mut MaintenanceResult, issue: CacheIssue) {
    if !result.issues.contains(&issue) {
        result.issues.push(issue);
    }
}

/// Remove one already-recognized candidate without following links.
fn remove_candidate(path: &Path, result: &mut MaintenanceResult) {
    let size = layout::path_size(path).unwrap_or(0);
    match layout::remove_no_follow(path) {
        Ok(()) => {
            result.removed_entries += 1;
            result.removed_bytes = result.removed_bytes.saturating_add(size);
        }
        Err(error) => result.issues.push(CacheIssue::new(path, error.to_string())),
    }
}

/// Convert wall-clock time to Unix seconds.
fn unix_seconds(time: SystemTime) -> Result<u64> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| RuskelError::Generate("System time is before the Unix epoch".to_string()))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Barrier};

    use tempfile::tempdir;

    use super::*;

    /// Build deterministic hooks for small on-disk fixtures.
    fn hooks(now: u64, high: u64, target: u64) -> MaintenanceHooks {
        MaintenanceHooks {
            now: Arc::new(move || UNIX_EPOCH + Duration::from_secs(now)),
            available_space: Arc::new(|_| Ok(u64::MAX)),
            routine_interval: Duration::from_secs(10),
            toolchain_retention: Duration::from_secs(20),
            workspace_retention: Duration::from_secs(30),
            high_water_bytes: high,
            eviction_target_bytes: target,
            low_space_bytes: 10,
            gc_wait_observer: None,
        }
    }

    /// Create one toolchain and workspace fixture.
    fn entry(layout: &CacheLayout, toolchain: &str, workspace: &str, last: u64, bytes: usize) {
        let toolchain_path = layout.build_dir().join(toolchain);
        let workspace_path = toolchain_path.join(workspace);
        fs::create_dir_all(&workspace_path).expect("workspace fixture directory");
        layout::write_timestamp(&toolchain_path.join(LAST_USE), last).expect("toolchain timestamp");
        layout::write_timestamp(&workspace_path.join(LAST_USE), last).expect("workspace timestamp");
        fs::write(workspace_path.join("artifact"), vec![0_u8; bytes]).expect("fixture artifact");
    }

    #[test]
    fn retention_removes_old_entries_and_keeps_future_metadata() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let old_toolchain = "a".repeat(64);
        let current_toolchain = "b".repeat(64);
        let old_workspace = "c".repeat(64);
        let future_workspace = "d".repeat(64);
        entry(&layout, &old_toolchain, &old_workspace, 1, 4);
        entry(&layout, &current_toolchain, &old_workspace, 1, 4);
        entry(&layout, &current_toolchain, &future_workspace, 200, 4);

        let result = run_pass(
            &layout,
            &hooks(100, u64::MAX, u64::MAX),
            Some(&current_toolchain),
            PassMode::Synchronous,
        )?;

        assert!(result.ran);
        assert!(!layout.build_dir().join(old_toolchain).exists());
        assert!(
            !layout
                .build_dir()
                .join(&current_toolchain)
                .join(old_workspace)
                .exists()
        );
        assert!(
            layout
                .build_dir()
                .join(current_toolchain)
                .join(future_workspace)
                .exists()
        );
        assert!(
            result
                .issues
                .iter()
                .any(|issue| issue.message().contains("future"))
        );
        Ok(())
    }

    #[test]
    fn budget_retains_newest_and_skips_locked_workspace() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let toolchain = "e".repeat(64);
        let oldest = "1".repeat(64);
        let locked = "2".repeat(64);
        let newest = "3".repeat(64);
        entry(&layout, &toolchain, &oldest, 80, 64);
        entry(&layout, &toolchain, &locked, 90, 64);
        entry(&layout, &toolchain, &newest, 100, 64);
        let locked_lease = layout.lock_exclusive(&layout.workspace_lock(&locked))?;

        let result = run_pass(
            &layout,
            &hooks(100, 1, 0),
            Some(&toolchain),
            PassMode::Synchronous,
        )?;

        assert!(result.ran);
        assert!(!layout.build_dir().join(&toolchain).join(oldest).exists());
        assert!(layout.build_dir().join(&toolchain).join(locked).exists());
        assert!(layout.build_dir().join(&toolchain).join(newest).exists());
        drop(locked_lease);
        Ok(())
    }

    #[test]
    fn worker_coalesces_signals_and_drains_on_shutdown() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let worker = MaintenanceWorker::start_with_hooks(layout.clone(), hooks(100, 1, 0))?;
        let current = "f".repeat(64);
        worker.signal(&current, false);
        worker.signal(&current, true);
        worker.shutdown();
        assert!(layout.maintenance_stamp().is_file());
        Ok(())
    }

    #[test]
    fn maintenance_finishes_interrupted_trash_deletion() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let trash_name = format!("{}.{}.1.1", "a".repeat(64), "b".repeat(64));
        let trash = layout.trash_dir().join(trash_name);
        fs::create_dir(&trash)?;
        fs::write(trash.join("artifact"), b"data")?;

        let result = run_pass(
            &layout,
            &hooks(100, u64::MAX, u64::MAX),
            None,
            PassMode::Synchronous,
        )?;

        assert_eq!(result.removed_entries, 1);
        assert!(!trash.exists());
        Ok(())
    }

    #[test]
    fn synchronous_recovery_waits_for_active_gc_without_duplicate_pass() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let gc = layout.lock_exclusive(&layout.gc_lock())?;
        let barrier = Arc::new(Barrier::new(2));
        let mut test_hooks = hooks(100, 1, 0);
        test_hooks.gc_wait_observer = Some(Arc::clone(&barrier));
        let thread_layout = layout.clone();
        let handle = thread::spawn(move || {
            run_pass(&thread_layout, &test_hooks, None, PassMode::Synchronous)
        });
        barrier.wait();
        drop(gc);
        let result = handle.join().expect("synchronous recovery thread")?;
        assert!(result.waited);
        assert!(!result.ran);
        assert!(!layout.maintenance_stamp().exists());
        Ok(())
    }
}
