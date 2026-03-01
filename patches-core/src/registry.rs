use std::collections::HashMap;

use crate::module::{InstanceId, Module};

/// A map from [`InstanceId`] to `Box<dyn Module>` used to preserve module
/// state across plan rebuilds.
///
/// When the [`Planner`](patches_engine::Planner) rebuilds a patch after a graph
/// change, it calls [`ExecutionPlan::into_registry`] on the previous plan to
/// move all module instances into a registry. `build_patch` then checks this
/// registry for each module in the new graph: if a matching `InstanceId` is
/// found the old (stateful) instance is reused instead of the fresh one.
///
/// Modules that are in the registry but not in the new graph are simply dropped
/// when the registry goes out of scope.
#[derive(Default)]
pub struct ModuleInstanceRegistry {
    instances: HashMap<InstanceId, Box<dyn Module>>,
}

impl ModuleInstanceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    /// Insert a module, keyed by its [`InstanceId`].
    ///
    /// If a module with the same `InstanceId` already exists it is replaced.
    pub fn insert(&mut self, module: Box<dyn Module>) {
        self.instances.insert(module.instance_id(), module);
    }

    /// Remove and return the module with the given `InstanceId`, if present.
    pub fn take(&mut self, id: InstanceId) -> Option<Box<dyn Module>> {
        self.instances.remove(&id)
    }

    /// Iterate over all `InstanceId`s currently held in the registry.
    pub fn instance_ids(&self) -> impl Iterator<Item = InstanceId> + '_ {
        self.instances.keys().copied()
    }

    /// Return `true` if the registry holds no modules.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}
