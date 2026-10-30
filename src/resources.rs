//! Read-only process/system memory observations and conservative admission policy.
//!
//! These are sampled tripwires, not allocator limits or guarantees about peak RAM.
//! Other workers, queries, graphics and history may allocate between observations.
use serde::Serialize;
use std::time::Duration;
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROBE_INTERVAL: Duration = Duration::from_millis(250);
pub const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(2);
const MIB: u64 = 1024 * 1024;

/// Separate from estimated inventory accounting: private committed process bytes
/// include the rest of this application too. Reserve is deliberately left unused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryBudget {
    pub max_private_bytes: u64,
    pub min_system_available_bytes: u64,
    pub process_reserve_bytes: u64,
}
impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            max_private_bytes: 2048 * MIB,
            min_system_available_bytes: 256 * MIB,
            process_reserve_bytes: 64 * MIB,
        }
    }
}
impl MemoryBudget {
    pub fn helpdesk() -> Self {
        Self {
            max_private_bytes: 768 * MIB,
            min_system_available_bytes: 512 * MIB,
            process_reserve_bytes: 64 * MIB,
        }
    }

    /// Unknown or stale measurements fail closed. Equality reserves the entire
    /// configured margin; saturating arithmetic also refuses invalid budgets.
    pub fn refusal(&self, observation: &ResourceObservation) -> Option<String> {
        if observation.age_ms > MAX_OBSERVATION_AGE.as_millis() as u64 {
            return Some(
                "Memory measurement stale; fresh process/system headroom is unknown".into(),
            );
        }
        let measured = match &observation.measured {
            Ok(measured) => measured,
            Err(ProbeError::Unavailable(reason)) => {
                return Some(format!("Memory measurement unavailable: {reason}"));
            }
        };
        if measured.private_bytes
            >= self
                .max_private_bytes
                .saturating_sub(self.process_reserve_bytes)
        {
            return Some(format!(
                "Memory pressure: process private bytes {} reached admission threshold {} (budget {}, reserve {})",
                measured.private_bytes,
                self.max_private_bytes
                    .saturating_sub(self.process_reserve_bytes),
                self.max_private_bytes,
                self.process_reserve_bytes,
            ));
        }
        if measured.system_available_bytes <= self.min_system_available_bytes {
            return Some(format!(
                "Memory pressure: system available bytes {} reached minimum {}",
                measured.system_available_bytes, self.min_system_available_bytes,
            ));
        }
        None
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum ProbeError {
    Unavailable(&'static str),
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryObservation {
    /// PROCESS_MEMORY_COUNTERS_EX.PrivateUsage: private commit, not working set.
    pub private_bytes: u64,
    /// MEMORYSTATUSEX.ullAvailPhys: system physical memory available at the probe.
    pub system_available_bytes: u64,
    pub observed_utc_epoch_ms: Option<u64>,
    pub source: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourceObservation {
    pub measured: Result<MemoryObservation, ProbeError>,
    /// Monotonic age at retrieval, independent of wall-clock adjustments.
    pub age_ms: u64,
}

/// Quick read-only calls; callers must run on a background sampler and rate limit.
#[cfg(windows)]
pub fn observe_native() -> Result<MemoryObservation, ProbeError> {
    use windows_sys::Win32::System::{
        ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetCurrentProcess,
    };
    // SAFETY: both structures are POD; each API receives its documented size and
    // a valid writable pointer. The pseudo handle is current-process/read-only and
    // must not be closed. Neither API requires enumeration or process elevation.
    unsafe {
        let mut system: MEMORYSTATUSEX = std::mem::zeroed();
        system.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut system) == 0 {
            return Err(ProbeError::Unavailable("GlobalMemoryStatusEx failed"));
        }
        let mut process: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        process.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut process as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            process.cb,
        ) == 0
        {
            return Err(ProbeError::Unavailable("K32GetProcessMemoryInfo failed"));
        }
        Ok(MemoryObservation {
            private_bytes: process.PrivateUsage as u64,
            system_available_bytes: system.ullAvailPhys,
            observed_utc_epoch_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| elapsed.as_millis().try_into().ok()),
            source: "K32GetProcessMemoryInfo.PrivateUsage + GlobalMemoryStatusEx.ullAvailPhys",
        })
    }
}

#[cfg(not(windows))]
pub fn observe_native() -> Result<MemoryObservation, ProbeError> {
    Err(ProbeError::Unavailable(
        "native memory probe requires Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(private_bytes: u64, available: u64) -> ResourceObservation {
        ResourceObservation {
            measured: Ok(MemoryObservation {
                private_bytes,
                system_available_bytes: available,
                observed_utc_epoch_ms: Some(1),
                source: "test probe boundary",
            }),
            age_ms: 0,
        }
    }

    #[test]
    fn admission_requires_headroom_and_known_fresh_measurements() {
        let budget = MemoryBudget {
            max_private_bytes: 1000,
            process_reserve_bytes: 100,
            min_system_available_bytes: 200,
        };
        assert!(budget.refusal(&observation(899, 201)).is_none());
        assert!(
            budget
                .refusal(&observation(900, 201))
                .unwrap()
                .contains("process private")
        );
        assert!(
            budget
                .refusal(&observation(10, 200))
                .unwrap()
                .contains("system available")
        );
        let unknown = ResourceObservation {
            measured: Err(ProbeError::Unavailable("fixture probe failure")),
            age_ms: 0,
        };
        assert!(
            budget
                .refusal(&unknown)
                .unwrap()
                .contains("measurement unavailable")
        );
        let mut stale = observation(1, 1000);
        stale.age_ms = MAX_OBSERVATION_AGE.as_millis() as u64 + 1;
        assert!(budget.refusal(&stale).unwrap().contains("stale"));
    }

    #[cfg(windows)]
    #[test]
    fn native_probe_reads_real_current_process_and_system_memory() {
        let sample = observe_native().expect("read-only Windows memory probe");
        assert!(sample.private_bytes > 0);
        assert!(sample.system_available_bytes > 0);
        assert!(sample.observed_utc_epoch_ms.is_some());
        assert!(sample.source.contains("GlobalMemoryStatusEx"));
        assert!(sample.source.contains("K32GetProcessMemoryInfo"));
    }
}
