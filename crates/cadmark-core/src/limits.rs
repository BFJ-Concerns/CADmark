// The ceilings every script execution runs under. Both defaults are
// user-adjustable in settings.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Wall-clock and resident-memory ceilings for one script execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// The script is stopped when it has run this long.
    pub wall_clock: Duration,
    /// The script is stopped when the kernel process's resident memory
    /// exceeds this many bytes. Resident, not address space: the modelling
    /// runtime reserves several gigabytes of address space at import.
    pub memory_bytes: u64,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(120),
            memory_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}

/// Which ceiling a stopped script crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitHit {
    WallClock,
    Memory,
}

impl LimitHit {
    /// The sentence the AI and the user read when a script is stopped.
    pub fn describe(self, limits: &ExecutionLimits) -> String {
        match self {
            Self::WallClock => format!(
                "The script was stopped after running for {} s (the wall-clock limit). \
                 It must finish sooner.",
                limits.wall_clock.as_secs()
            ),
            Self::Memory => format!(
                "The script was stopped for using more than {} MB of memory (the memory limit). \
                 It must use less.",
                limits.memory_bytes / (1024 * 1024)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_ceilings() {
        let limits = ExecutionLimits::default();
        assert_eq!(limits.wall_clock, Duration::from_secs(120));
        assert_eq!(limits.memory_bytes, 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn limit_descriptions_name_the_ceiling_crossed() {
        let limits = ExecutionLimits::default();
        assert!(LimitHit::WallClock.describe(&limits).contains("120 s"));
        assert!(LimitHit::Memory.describe(&limits).contains("4096 MB"));
    }
}
