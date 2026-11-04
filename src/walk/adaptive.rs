//! Detects that a walk worker spends its time waiting for the file system, which is when more
//! workers help: on a cold cache or a slow (network) file system each worker has at most one
//! request in flight.
//!
//! A worker adds up the wall time of its tasks. Once that reaches a window, it compares it with
//! the CPU time of the thread and counts the voluntary context switches in the window. Waiting
//! for I/O is a voluntary switch; being preempted by another process is not, so a busy machine
//! does not look like a slow file system. Only Linux offers both numbers per thread
//! (`getrusage` with `RUSAGE_THREAD`); elsewhere the walk keeps the number of workers it was
//! given.

use std::time::Duration;

/// When a window counts as blocked on I/O.
#[derive(Clone, Copy)]
pub struct Policy {
    /// Wall time of tasks after which a window is judged.
    pub window: Duration,
    /// Voluntary context switches a window needs, not counting waits for work.
    pub min_switches: u64,
    /// Share of the window's wall time that was not CPU time.
    pub min_blocked: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            window: Duration::from_millis(5),
            min_switches: 8,
            min_blocked: 0.5,
        }
    }
}

/// Measures one worker.
pub struct Meter {
    policy: Policy,
    busy: Duration,
    cpu: Duration,
    switches: u64,
    /// Waits for work since the last judgment; each is a voluntary switch.
    waits: u64,
}

impl Meter {
    // Only the stub of `thread_usage` outside Linux is `const`.
    #[cfg_attr(not(target_os = "linux"), allow(clippy::missing_const_for_fn))]
    pub fn new(policy: Policy) -> Self {
        let (cpu, switches) = thread_usage();
        Self {
            policy,
            busy: Duration::ZERO,
            cpu,
            switches,
            waits: 0,
        }
    }

    /// Notes that the worker waited for work.
    pub const fn waited(&mut self) {
        self.waits += 1;
    }

    /// Adds a task that took `wall`. Returns whether the window it completes was blocked.
    pub fn task(&mut self, wall: Duration) -> bool {
        self.busy += wall;
        if self.busy < self.policy.window {
            return false;
        }

        let (now_cpu, now_switches) = thread_usage();
        let used = now_cpu.saturating_sub(self.cpu);
        let voluntary = now_switches
            .saturating_sub(self.switches)
            .saturating_sub(self.waits);
        let blocked = self.busy.saturating_sub(used).as_secs_f64()
            >= self.policy.min_blocked * self.busy.as_secs_f64();

        self.busy = Duration::ZERO;
        self.cpu = now_cpu;
        self.switches = now_switches;
        self.waits = 0;

        blocked && voluntary >= self.policy.min_switches
    }
}

/// CPU time and voluntary context switches of the calling thread.
#[cfg(target_os = "linux")]
fn thread_usage() -> (Duration, u64) {
    // SAFETY: `getrusage` fills the zeroed struct it is given.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_THREAD, &raw mut usage) != 0 {
            return (Duration::ZERO, 0);
        }
        usage
    };

    let time = |t: libc::timeval| {
        Duration::new(
            u64::try_from(t.tv_sec).unwrap_or(0),
            u32::try_from(t.tv_usec).unwrap_or(0) * 1000,
        )
    };

    (
        time(usage.ru_utime) + time(usage.ru_stime),
        u64::try_from(usage.ru_nvcsw).unwrap_or(0),
    )
}

#[cfg(not(target_os = "linux"))]
const fn thread_usage() -> (Duration, u64) {
    (Duration::ZERO, 0)
}
