use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// A stable, unique identifier assigned to a module instance at construction time.
///
/// `InstanceId` is immutable for the lifetime of the module and survives across
/// plan rebuilds, allowing the planner to match surviving modules to their pool
/// slots in a new plan.
///
/// IDs are generated from a global atomic counter; no two independently constructed
/// modules will share an `InstanceId` within a single process run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(u64);

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceId({})", self.0)
    }
}

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(0);

impl InstanceId {
    /// Allocate a fresh `InstanceId`. Each call returns a distinct value.
    pub fn next() -> Self {
        Self(NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Return the raw counter value. Useful for seeding PRNGs; note the first
    /// allocated ID has value `0`, so callers that need a non-zero seed should
    /// add an offset (e.g. `as_u64() + 1`).
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_are_unique() {
        let a = InstanceId::next();
        let b = InstanceId::next();
        assert_ne!(a, b);
    }
}