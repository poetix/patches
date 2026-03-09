use std::collections::{HashMap, HashSet};

use crate::modules::InstanceId;
use super::graph_index::GraphIndex;
use super::super::graph::NodeId;
use super::PlanError;

// ── BufferAllocState ──────────────────────────────────────────────────────────

/// Stable buffer index allocation state threaded across successive plan builds.
///
/// `BufferAllocState` allows cables that share a `(NodeId, output_port_index)` key
/// across re-plans to reuse the same pool slot, so the audio thread reads/writes the
/// same memory before and after a plan swap.
///
/// The `Default` implementation starts the high-water mark at `1`, reserving slot `0`
/// as the permanent-zero slot.
pub struct BufferAllocState {
    /// Maps `(NodeId, output_port_index)` to a stable buffer pool index.
    pub output_buf: HashMap<(NodeId, usize), usize>,
    /// Recycled buffer indices available for reuse (LIFO via [`Vec::pop`]).
    pub freelist: Vec<usize>,
    /// High-water mark: the next index to allocate when the freelist is empty.
    /// Starts at `1` so that index `0` remains the permanent-zero slot.
    pub next_hwm: usize,
}

impl Default for BufferAllocState {
    fn default() -> Self {
        Self {
            output_buf: HashMap::new(),
            freelist: Vec::new(),
            next_hwm: 1,
        }
    }
}

// ── ModuleAllocState / ModuleAllocDiff ────────────────────────────────────────

/// Stable module slot allocation state threaded across successive plan builds.
///
/// `ModuleAllocState` is the control-thread mirror of the audio thread's module pool,
/// analogous to [`BufferAllocState`] for the buffer pool. It tracks which pool slot each
/// [`InstanceId`] occupies so that surviving modules reuse their slots across re-plans.
///
/// The `Default` implementation starts the high-water mark at `0` (no permanent-zero slot
/// is needed for modules).
#[derive(Default)]
pub struct ModuleAllocState {
    /// Maps [`InstanceId`] to the pool slot index currently holding that module.
    pub pool_map: HashMap<InstanceId, usize>,
    /// Recycled slot indices available for reuse (LIFO via [`Vec::pop`]).
    pub freelist: Vec<usize>,
    /// High-water mark: the next index to allocate when the freelist is empty.
    /// Starts at `0`.
    pub next_hwm: usize,
}

/// Result of [`ModuleAllocState::diff`]: the new pool map and freelist after applying
/// the module set for the next graph.
#[derive(Debug)]
pub struct ModuleAllocDiff {
    /// Slot index for each [`InstanceId`] in the new graph (surviving + newly allocated).
    pub slot_map: HashMap<InstanceId, usize>,
    /// Updated freelist (surviving freelisted indices + newly tombstoned slots).
    pub freelist: Vec<usize>,
    /// New high-water mark.
    pub next_hwm: usize,
    /// Slot indices that were tombstoned (freed) by this diff.
    pub tombstoned: Vec<usize>,
}

impl ModuleAllocState {
    /// Compute allocation changes given the set of [`InstanceId`]s for the incoming graph.
    ///
    /// - **Surviving** entries: already in `pool_map` → reuse their existing slot index.
    /// - **New** entries: not in `pool_map` → acquired from `freelist` (LIFO) or `next_hwm`.
    ///   Returns [`PlanError::ModulePoolExhausted`] if the index would reach `capacity`.
    /// - **Tombstoned** entries: in `pool_map` but not in `new_ids` → slot returned to freelist.
    pub fn diff(
        &self,
        new_ids: &HashSet<InstanceId>,
        capacity: usize,
    ) -> Result<ModuleAllocDiff, PlanError> {
        let mut slot_map: HashMap<InstanceId, usize> = HashMap::new();
        let mut freelist: Vec<usize> = self.freelist.clone();
        let mut next_hwm: usize = self.next_hwm;
        let mut tombstoned: Vec<usize> = Vec::new();

        // Tombstone: entries in the old pool_map that are not in the new set.
        for (&id, &slot) in &self.pool_map {
            if !new_ids.contains(&id) {
                freelist.push(slot);
                tombstoned.push(slot);
            }
        }

        // Allocate: surviving entries reuse their slot; new entries get a fresh one.
        for &id in new_ids {
            if let Some(&existing) = self.pool_map.get(&id) {
                slot_map.insert(id, existing);
            } else {
                let idx = if let Some(recycled) = freelist.pop() {
                    recycled
                } else {
                    let idx = next_hwm;
                    next_hwm += 1;
                    idx
                };
                if idx >= capacity {
                    return Err(PlanError::ModulePoolExhausted);
                }
                slot_map.insert(id, idx);
            }
        }

        Ok(ModuleAllocDiff { slot_map, freelist, next_hwm, tombstoned })
    }
}

// ── BufferAllocation ──────────────────────────────────────────────────────────

/// Result of the buffer allocation phase, passed into the action phase.
pub struct BufferAllocation {
    pub output_buf: HashMap<(NodeId, usize), usize>,
    pub to_zero: Vec<usize>,
    pub freelist: Vec<usize>,
    pub next_hwm: usize,
}

// ── allocate_buffers ──────────────────────────────────────────────────────────

/// Assign stable cable buffer pool indices for `order`.
///
/// Reuses any `(NodeId, port_idx)` key already present in `prev_alloc`.
/// New keys are filled from the freelist (LIFO) or the high-water mark.
/// Old keys absent from the new graph are returned to the freelist and marked
/// for zeroing on plan adoption.
///
/// Returns [`PlanError::BufferPoolExhausted`] if the index would reach `pool_capacity`.
pub fn allocate_buffers(
    index: &GraphIndex<'_>,
    order: &[NodeId],
    prev_alloc: &BufferAllocState,
    pool_capacity: usize,
) -> Result<BufferAllocation, PlanError> {
    let mut freelist = prev_alloc.freelist.clone();
    let mut next_hwm = prev_alloc.next_hwm;
    let mut to_zero = Vec::new();
    let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

    for id in order {
        let desc = &index
            .get_node(id)
            .ok_or_else(|| PlanError::Internal(format!("node {id:?} missing from graph")))?
            .module_descriptor;

        for (port_idx, _) in desc.outputs.iter().enumerate() {
            let key = (id.clone(), port_idx);
            if let Some(&existing) = prev_alloc.output_buf.get(&key) {
                output_buf.insert(key, existing);
            } else {
                let idx = freelist.pop().unwrap_or_else(|| {
                    let i = next_hwm;
                    next_hwm += 1;
                    i
                });
                if idx >= pool_capacity {
                    return Err(PlanError::BufferPoolExhausted);
                }
                to_zero.push(idx);
                output_buf.insert(key, idx);
            }
        }
    }

    // Deallocate ports present in the old alloc that are not in the new graph.
    for (key, &buf_idx) in &prev_alloc.output_buf {
        if !output_buf.contains_key(key) {
            to_zero.push(buf_idx);
            freelist.push(buf_idx);
        }
    }

    Ok(BufferAllocation { output_buf, to_zero, freelist, next_hwm })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use super::super::PlanError;
    use crate::modules::InstanceId;

    fn fresh_ids(n: usize) -> Vec<InstanceId> {
        (0..n).map(|_| InstanceId::next()).collect()
    }

    fn id_set(ids: &[InstanceId]) -> HashSet<InstanceId> {
        ids.iter().copied().collect()
    }

    fn apply(diff: &ModuleAllocDiff) -> ModuleAllocState {
        ModuleAllocState {
            pool_map: diff.slot_map.clone(),
            freelist: diff.freelist.clone(),
            next_hwm: diff.next_hwm,
        }
    }

    // ── slot_map completeness ─────────────────────────────────────────────────

    /// `slot_map` contains exactly the ids in `new_ids` — no more, no less.
    #[test]
    fn slot_map_contains_exactly_new_ids() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(4);
        let diff = state.diff(&id_set(&ids), 64).unwrap();

        assert_eq!(diff.slot_map.len(), 4);
        for id in &ids {
            assert!(diff.slot_map.contains_key(id), "id missing from slot_map");
        }
    }

    /// All slots assigned to fresh ids are distinct.
    #[test]
    fn fresh_slots_are_unique() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(5);
        let diff = state.diff(&id_set(&ids), 64).unwrap();

        let mut slots: Vec<usize> = diff.slot_map.values().copied().collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), 5, "all assigned slots must be distinct");
    }

    // ── empty inputs ──────────────────────────────────────────────────────────

    /// Diffing an empty set against an empty state is a no-op.
    #[test]
    fn empty_diff_on_empty_state_is_noop() {
        let state = ModuleAllocState::default();
        let diff = state.diff(&HashSet::new(), 64).unwrap();

        assert!(diff.slot_map.is_empty());
        assert!(diff.tombstoned.is_empty());
        assert!(diff.freelist.is_empty());
        assert_eq!(diff.next_hwm, 0);
    }

    /// Diffing an empty set against a non-empty state tombstones everything.
    #[test]
    fn empty_new_ids_tombstones_all() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(3);
        let diff0 = state.diff(&id_set(&ids), 64).unwrap();
        let hwm = diff0.next_hwm;

        let state1 = apply(&diff0);
        let diff1 = state1.diff(&HashSet::new(), 64).unwrap();

        assert!(diff1.slot_map.is_empty());
        assert_eq!(diff1.tombstoned.len(), 3, "all three slots must be tombstoned");
        assert_eq!(diff1.freelist.len(), 3, "all three slots must be freelisted");
        assert_eq!(diff1.next_hwm, hwm, "hwm must not change");
    }

    // ── capacity boundary ─────────────────────────────────────────────────────

    /// Allocating exactly at capacity (slots 0..capacity-1) succeeds.
    #[test]
    fn allocation_at_capacity_boundary_succeeds() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(3); // slots 0, 1, 2 — fits exactly in capacity 3
        let result = state.diff(&id_set(&ids), 3);
        assert!(result.is_ok(), "allocation filling capacity exactly must succeed");
        assert_eq!(result.unwrap().next_hwm, 3);
    }

    /// Allocating one past capacity fails.
    #[test]
    fn allocation_one_past_capacity_fails() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(3); // needs slots 0, 1, 2 but capacity is 2
        let result = state.diff(&id_set(&ids), 2);
        assert!(
            matches!(result, Err(PlanError::ModulePoolExhausted)),
            "allocating beyond capacity must return ModulePoolExhausted"
        );
    }

    /// Recycling from the freelist does not consume HWM and does not trigger exhaustion.
    #[test]
    fn recycled_slot_does_not_count_against_capacity() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(2); // slots 0, 1 — capacity 2 exactly filled
        let diff0 = state.diff(&id_set(&ids), 2).unwrap();

        // Remove both — slots 0 and 1 land on the freelist.
        let state1 = apply(&diff0);
        let diff1 = state1.diff(&HashSet::new(), 2).unwrap();

        // Re-add two new modules — must recycle from freelist without exceeding capacity.
        let new_ids = fresh_ids(2);
        let state2 = apply(&diff1);
        let diff2 = state2.diff(&id_set(&new_ids), 2).unwrap();
        assert_eq!(diff2.next_hwm, 2, "hwm must not grow when recycling");
    }

    // ── LIFO freelist ordering ────────────────────────────────────────────────

    /// The last slot pushed onto the freelist is the first one recycled.
    #[test]
    fn freelist_is_lifo() {
        // Allocate three slots then tombstone all of them.
        let state = ModuleAllocState::default();
        let ids = fresh_ids(3);
        let diff0 = state.diff(&id_set(&ids), 64).unwrap();

        let state1 = apply(&diff0);
        let diff1 = state1.diff(&HashSet::new(), 64).unwrap();
        let last_on_freelist = *diff1.freelist.last().unwrap();

        // Introduce a single new id — must pop from the freelist (LIFO).
        let new_id = fresh_ids(1)[0];
        let state2 = apply(&diff1);
        let diff2 = state2.diff(&id_set(&[new_id]), 64).unwrap();

        assert_eq!(
            diff2.slot_map[&new_id], last_on_freelist,
            "new module must reuse the last slot pushed onto the freelist (LIFO)"
        );
        assert_eq!(diff2.freelist.len(), 2, "two slots remain on freelist after recycling one");
    }

    // ── freelist accounting ───────────────────────────────────────────────────

    /// freelist after diff == old_freelist + tombstoned - recycled.
    ///
    /// With a pre-existing freelist entry (slot 5) and two new ids, one
    /// recycles slot 5 and the other advances the HWM to slot 6.
    #[test]
    fn freelist_accounting_is_correct() {
        let state = ModuleAllocState {
            pool_map: std::collections::HashMap::new(),
            freelist: vec![5],
            next_hwm: 6,
        };

        let ids = fresh_ids(2);
        let diff = state.diff(&id_set(&ids), 64).unwrap();

        assert!(diff.freelist.is_empty(), "freelist must be empty after recycling the one entry");
        assert_eq!(diff.next_hwm, 7, "hwm advanced once for the non-recycled id");
        assert!(diff.tombstoned.is_empty());

        let mut slots: Vec<usize> = diff.slot_map.values().copied().collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![5, 6], "must contain the recycled slot and the new hwm slot");
    }

    // ── surviving entries ─────────────────────────────────────────────────────

    /// Surviving entries are absent from `tombstoned` and keep their slot.
    #[test]
    fn surviving_entries_not_tombstoned() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(3);
        let diff0 = state.diff(&id_set(&ids), 64).unwrap();

        // Remove only ids[2]; ids[0] and ids[1] survive.
        let state1 = apply(&diff0);
        let diff1 = state1.diff(&id_set(&ids[..2]), 64).unwrap();

        assert_eq!(diff1.tombstoned.len(), 1, "only the removed id is tombstoned");
        assert!(diff1.tombstoned.contains(&diff0.slot_map[&ids[2]]));

        for id in &ids[..2] {
            assert_eq!(diff0.slot_map[id], diff1.slot_map[id], "surviving slot must be stable");
        }
    }

    /// `tombstoned` and `slot_map` values are disjoint.
    #[test]
    fn tombstoned_and_slot_map_are_disjoint() {
        let state = ModuleAllocState::default();
        let ids = fresh_ids(4);
        let diff0 = state.diff(&id_set(&ids), 64).unwrap();

        let state1 = apply(&diff0);
        let diff1 = state1.diff(&id_set(&ids[..2]), 64).unwrap();

        let active_slots: HashSet<usize> = diff1.slot_map.values().copied().collect();
        for &t in &diff1.tombstoned {
            assert!(!active_slots.contains(&t), "tombstoned slot {t} must not appear in slot_map");
        }
    }
}
