use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::{InstanceId, ModuleDescriptor, ModuleGraph, ModuleShape, Node, NodeId, PortConnectivity};
use crate::parameter_map::ParameterMap;

// ── PlanError ─────────────────────────────────────────────────────────────────

/// Errors that can occur during the decision phase of plan building.
#[derive(Debug)]
pub enum PlanError {
    /// The number of modules would exceed the module pool capacity.
    ModulePoolExhausted,
    /// An internal consistency invariant was violated (indicates a bug in the builder).
    InternalError(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::ModulePoolExhausted => {
                write!(f, "module pool exhausted: too many modules")
            }
            PlanError::InternalError(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for PlanError {}

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

// ── NodeState ─────────────────────────────────────────────────────────────────

/// Per-node identity and parameter state carried across successive builds.
pub struct NodeState {
    /// The module type name (from `ModuleDescriptor::module_name`).
    pub module_name: &'static str,
    /// Stable identity assigned by the planner when this node first appeared.
    pub instance_id: InstanceId,
    /// The parameter map applied to this node during the last build.
    pub parameter_map: ParameterMap,
    /// The shape used when this module instance was created.
    ///
    /// If the shape changes on the next build (same `NodeId`, same module type),
    /// the old instance is tombstoned and a fresh one is created with the new shape.
    pub shape: ModuleShape,
    /// The port connectivity computed during the last build.
    ///
    /// Stored so that the engine can diff against it to emit connectivity updates only
    /// when the wiring actually changes.
    pub connectivity: PortConnectivity,
}

// ── PlannerState ──────────────────────────────────────────────────────────────

/// Planning state threaded across successive plan builds.
///
/// `PlannerState` records node identity, buffer allocation, and module slot
/// allocation. Passing the previous build's state into the next call enables
/// graph diffing: surviving nodes reuse their `InstanceId` and pool slot;
/// only added and type-changed nodes trigger module instantiation.
pub struct PlannerState {
    /// Maps each [`NodeId`] to its last-known identity and parameters.
    pub nodes: HashMap<NodeId, NodeState>,
    /// Stable buffer index allocation carried across builds.
    pub buffer_alloc: BufferAllocState,
    /// Stable module slot allocation carried across builds.
    pub module_alloc: ModuleAllocState,
}

impl PlannerState {
    /// Return an empty state for the first build.
    ///
    /// Using an empty state causes every node in the graph to be treated as
    /// new: each receives a fresh [`InstanceId`] and a new module is
    /// instantiated via the registry.
    pub fn empty() -> Self {
        Self {
            nodes: HashMap::new(),
            buffer_alloc: BufferAllocState::default(),
            module_alloc: ModuleAllocState::default(),
        }
    }
}

// ── Type aliases ──────────────────────────────────────────────────────────────

pub(crate) type EdgeList = Vec<(NodeId, String, u32, NodeId, String, u32, f64)>;
type InputBufferMap = HashMap<(NodeId, String, u32), (usize, f64)>;

// ── GraphIndex ────────────────────────────────────────────────────────────────

/// Pre-built connectivity index over a [`ModuleGraph`].
///
/// Constructed once from the graph's edge list, enabling O(1) per-port
/// connectivity queries. Used by the decision phase and action phase of plan building.
pub struct GraphIndex<'a> {
    graph: &'a ModuleGraph,
    edges: EdgeList,
    connected_inputs: HashSet<(NodeId, String, u32)>,
    connected_outputs: HashSet<(NodeId, String, u32)>,
}

impl<'a> GraphIndex<'a> {
    pub fn build(graph: &'a ModuleGraph) -> Self {
        let edges = graph.edge_list();
        let mut connected_inputs = HashSet::with_capacity(edges.len());
        let mut connected_outputs = HashSet::with_capacity(edges.len());
        for (from, out_name, out_idx, to, in_name, in_idx, _) in &edges {
            connected_inputs.insert((to.clone(), in_name.clone(), *in_idx));
            connected_outputs.insert((from.clone(), out_name.clone(), *out_idx));
        }
        Self { graph, edges, connected_inputs, connected_outputs }
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&'a Node> {
        self.graph.get_node(id)
    }

    /// Compute [`PortConnectivity`] for a single node using this index.
    ///
    /// An input port is marked connected if the index contains `(node_id, name, idx)`.
    /// An output port is marked connected if the index contains `(node_id, name, idx)`.
    /// Each port lookup is O(1); total cost is O(P_in + P_out) per node.
    pub fn compute_connectivity(
        &self,
        desc: &ModuleDescriptor,
        node_id: &NodeId,
    ) -> PortConnectivity {
        let mut connectivity = PortConnectivity::new(desc.inputs.len(), desc.outputs.len());
        for (i, port) in desc.inputs.iter().enumerate() {
            if self.connected_inputs.contains(&(node_id.clone(), port.name.to_owned(), port.index)) {
                connectivity.inputs[i] = true;
            }
        }
        for (j, port) in desc.outputs.iter().enumerate() {
            if self.connected_outputs.contains(&(node_id.clone(), port.name.to_owned(), port.index)) {
                connectivity.outputs[j] = true;
            }
        }
        connectivity
    }
}

// ── ResolvedGraph ─────────────────────────────────────────────────────────────

/// A [`GraphIndex`] extended with a resolved input-buffer map.
///
/// Constructed after buffer allocation is complete; enables O(1) input-buffer
/// lookups per module port in the action phase.
pub struct ResolvedGraph<'a> {
    #[allow(dead_code)]
    index: &'a GraphIndex<'a>,
    input_buffer_map: InputBufferMap,
}

impl<'a> ResolvedGraph<'a> {
    pub fn build(
        index: &'a GraphIndex<'a>,
        output_buf: &HashMap<(NodeId, usize), usize>,
    ) -> Result<Self, PlanError> {
        let input_buffer_map = build_input_buffer_map(&index.edges, output_buf, index.graph)?;
        Ok(Self { index, input_buffer_map })
    }

    /// Resolve each input port of `desc` on `node_id` to a `(buffer_index, scale)` pair.
    ///
    /// Looks up each port in `input_buffer_map` in O(1). Unconnected ports default to
    /// `(0, 1.0)` — the permanent-zero slot with implicit scale 1.0.
    pub fn resolve_input_buffers(
        &self,
        desc: &ModuleDescriptor,
        node_id: &NodeId,
    ) -> Vec<(usize, f64)> {
        desc.inputs
            .iter()
            .map(|port| {
                self.input_buffer_map
                    .get(&(node_id.clone(), port.name.to_owned(), port.index))
                    .copied()
                    .unwrap_or((0, 1.0))
            })
            .collect()
    }
}

// ── build_input_buffer_map ────────────────────────────────────────────────────

/// Build a map from `(to_node, in_port_name, in_port_idx)` to `(buffer_slot, scale)`.
///
/// Performs one O(E) pass over the edge list. For each edge the source node is looked up
/// in `graph`, the output port is located by name and index within that node's descriptor,
/// and the pre-allocated buffer slot is retrieved from `output_buf`.
///
/// Returns [`PlanError::InternalError`] if a referenced source node, output port, or
/// buffer allocation is missing.
fn build_input_buffer_map(
    edges: &EdgeList,
    output_buf: &HashMap<(NodeId, usize), usize>,
    graph: &ModuleGraph,
) -> Result<InputBufferMap, PlanError> {
    let mut map = HashMap::with_capacity(edges.len());
    for (from, out_name, out_idx, to, in_name, in_idx, scale) in edges {
        let from_node = graph
            .get_node(from)
            .ok_or_else(|| PlanError::InternalError(format!("node {from:?} missing from graph")))?;
        let out_port_idx = from_node
            .module_descriptor
            .outputs
            .iter()
            .position(|p| p.name == out_name.as_str() && p.index == *out_idx)
            .ok_or_else(|| {
                PlanError::InternalError(format!(
                    "output port {out_name:?}/{out_idx} not found on node {from:?}"
                ))
            })?;
        let buf = output_buf
            .get(&(from.clone(), out_port_idx))
            .copied()
            .ok_or_else(|| {
                PlanError::InternalError(format!(
                    "buffer for ({from:?}, {out_port_idx}) not found"
                ))
            })?;
        map.insert((to.clone(), in_name.clone(), *in_idx), (buf, *scale));
    }
    Ok(map)
}

// ── Test helpers (cfg(test)) ──────────────────────────────────────────────────

/// Build a [`ResolvedGraph`] from a pre-built [`GraphIndex`] and a raw input-buffer map.
///
/// Bypasses [`build_input_buffer_map`] so tests can inject a custom map directly.
#[cfg(test)]
fn resolved_graph_for_test<'a>(
    index: &'a GraphIndex<'a>,
    input_buffer_map: InputBufferMap,
) -> ResolvedGraph<'a> {
    ResolvedGraph { index, input_buffer_map }
}

/// Build a [`GraphIndex`] from raw edge data without a populated [`ModuleGraph`].
///
/// The `graph` field is set to `graph` (may be empty); only the connectivity sets and
/// edge list are populated from `edges_raw`. Used in tests for `compute_connectivity`
/// where real module nodes are not required.
#[cfg(test)]
fn graph_index_for_test<'a>(
    graph: &'a ModuleGraph,
    edges_raw: &[(NodeId, String, u32, NodeId, String, u32, f64)],
) -> GraphIndex<'a> {
    let mut connected_inputs = HashSet::new();
    let mut connected_outputs = HashSet::new();
    for (from, out_name, out_idx, to, in_name, in_idx, _) in edges_raw {
        connected_inputs.insert((to.clone(), in_name.clone(), *in_idx));
        connected_outputs.insert((from.clone(), out_name.clone(), *out_idx));
    }
    GraphIndex {
        graph,
        edges: edges_raw.to_vec(),
        connected_inputs,
        connected_outputs,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::InstanceId;

    // ── helpers ───────────────────────────────────────────────────────────────

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

    // ── GraphIndex / ResolvedGraph tests (moved from patches-engine T-0103) ───

    use crate::{ModuleDescriptor, ModuleGraph, ModuleShape, PortDescriptor};

    fn two_node_graph() -> (ModuleGraph, NodeId, NodeId) {
        use crate::parameter_map::ParameterMap;
        let src_desc = ModuleDescriptor {
            module_name: "Src",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![],
            is_sink: false,
        };
        let dst_desc = ModuleDescriptor {
            module_name: "Dst",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![PortDescriptor { name: "in", index: 0 }],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        };
        let mut graph = ModuleGraph::new();
        graph.add_module("src", src_desc, &ParameterMap::new()).unwrap();
        graph.add_module("dst", dst_desc, &ParameterMap::new()).unwrap();
        let src_id = NodeId::from("src");
        let dst_id = NodeId::from("dst");
        (graph, src_id, dst_id)
    }

    fn two_port_desc() -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Test",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "in", index: 0 },
                PortDescriptor { name: "in", index: 1 },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0 },
                PortDescriptor { name: "out", index: 1 },
            ],
            parameters: vec![],
            is_sink: false,
        }
    }

    // ── resolve_input_buffers tests ───────────────────────────────────────────

    #[test]
    fn resolve_unconnected_port_returns_zero_buffer_scale_one() {
        let (graph, _, dst_id) = two_node_graph();
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, HashMap::new());

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(0, 1.0)], "unconnected port must map to (0, 1.0)");
    }

    #[test]
    fn resolve_connected_port_returns_correct_buffer_and_scale() {
        let (graph, _src_id, dst_id) = two_node_graph();
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();

        let mut map: HashMap<(NodeId, String, u32), (usize, f64)> = HashMap::new();
        map.insert((dst_id.clone(), "in".to_string(), 0), (7, 0.5));
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, map);

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(7, 0.5)], "connected port must resolve to buffer 7 scale 0.5");
    }

    #[test]
    fn resolve_multiple_ports_independently() {
        use crate::parameter_map::ParameterMap;
        let dst_desc_data = ModuleDescriptor {
            module_name: "Dst2",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "x", index: 0 },
                PortDescriptor { name: "y", index: 0 },
            ],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        };
        let mut graph = ModuleGraph::new();
        graph.add_module("dst2", dst_desc_data, &ParameterMap::new()).unwrap();
        let dst_id = NodeId::from("dst2");
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();

        let mut map: HashMap<(NodeId, String, u32), (usize, f64)> = HashMap::new();
        map.insert((dst_id.clone(), "x".to_string(), 0), (3, 1.0));
        map.insert((dst_id.clone(), "y".to_string(), 0), (4, 2.0));
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, map);

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(3, 1.0), (4, 2.0)]);
    }

    // ── build_input_buffer_map tests ──────────────────────────────────────────

    #[test]
    fn build_input_buffer_map_missing_source_node_returns_internal_error() {
        let (graph, _src_id, dst_id) = two_node_graph();

        let ghost_id = NodeId::from("ghost");
        let edges = vec![(
            ghost_id.clone(), "out".to_string(), 0u32,
            dst_id.clone(), "in".to_string(), 0u32,
            1.0f64,
        )];
        let output_buf = HashMap::new();

        let result = build_input_buffer_map(&edges, &output_buf, &graph);
        assert!(
            matches!(result, Err(PlanError::InternalError(_))),
            "missing source node must return InternalError"
        );
    }

    #[test]
    fn build_input_buffer_map_missing_buffer_returns_internal_error() {
        let (graph, src_id, dst_id) = two_node_graph();

        let edges = vec![(
            src_id.clone(), "out".to_string(), 0u32,
            dst_id.clone(), "in".to_string(), 0u32,
            1.0f64,
        )];
        let output_buf = HashMap::new();

        let result = build_input_buffer_map(&edges, &output_buf, &graph);
        assert!(
            matches!(result, Err(PlanError::InternalError(_))),
            "missing buffer allocation must return InternalError"
        );
    }

    // ── compute_connectivity tests ────────────────────────────────────────────

    #[test]
    fn connectivity_no_edges_all_false() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &[]);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_single_input_connected() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let other = NodeId::from("src");
        let edges = vec![(other, "out".to_string(), 0, node.clone(), "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(c.inputs[0]);
        assert!(!c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_single_output_connected() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let other = NodeId::from("dst");
        let edges = vec![(node.clone(), "out".to_string(), 1, other, "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(c.outputs[1]);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0]);
    }

    #[test]
    fn connectivity_multiple_ports_correct_subset() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        let edges = vec![
            (src.clone(), "out".to_string(), 0, node.clone(), "in".to_string(), 1, 1.0),
            (node.clone(), "out".to_string(), 0, dst.clone(), "in".to_string(), 0, 1.0),
        ];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && c.inputs[1]);
        assert!(c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_edges_for_other_nodes_ignored() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        let edges = vec![(a.clone(), "out".to_string(), 0, b.clone(), "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_no_false_positive_same_name_different_index() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let src = NodeId::from("src");
        let edges = vec![(src, "out".to_string(), 0, node.clone(), "in".to_string(), 1, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0], "in/0 must not be marked");
        assert!(c.inputs[1], "in/1 must be marked");
    }
}
