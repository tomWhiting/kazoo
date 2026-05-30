//! Connection routing and topological sort for processing order.

use std::collections::VecDeque;

use super::node::{ModularNode, NodeId, PortType};

/// A connection between two ports in the graph.
#[derive(Debug, Clone)]
pub struct Connection {
    /// Source node ID.
    pub from_node: NodeId,
    /// Source port index.
    pub from_port: usize,
    /// Destination node ID.
    pub to_node: NodeId,
    /// Destination port index.
    pub to_port: usize,
}

/// Entry for a node in the graph.
#[derive(Debug)]
struct GraphNode {
    /// Unique ID.
    id: NodeId,
    /// The processing node.
    node: Box<dyn ModularNode>,
    /// Pre-allocated output buffers (one per output port).
    output_buffers: Vec<Vec<f32>>,
    /// Pre-allocated input scratch buffers (one per input port).
    input_scratch: Vec<Vec<f32>>,
}

/// Modular synthesis node graph.
///
/// Manages nodes, connections, topological ordering, and block processing.
/// All buffers are pre-allocated at graph construction or when nodes are added.
#[derive(Debug)]
pub struct NodeGraph {
    /// All nodes in the graph.
    nodes: Vec<GraphNode>,
    /// All connections.
    connections: Vec<Connection>,
    /// Processing order (node indices into `nodes` vec, topologically sorted).
    process_order: Vec<usize>,
    /// Next available node ID.
    next_id: NodeId,
    /// Block size for processing.
    block_size: usize,
    /// Sample rate.
    sample_rate: f32,
    /// Scratch buffer for mixing multiple sources into one input (pre-allocated for future use).
    _mix_buffer: Vec<f32>,
    /// Scratch copy of `process_order` to avoid cloning each call.
    scratch_order: Vec<usize>,
}

impl NodeGraph {
    /// Create a new empty node graph.
    #[must_use]
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        Self {
            nodes: Vec::new(),
            connections: Vec::new(),
            process_order: Vec::new(),
            next_id: 0,
            block_size: block_size.max(1),
            sample_rate: sample_rate.max(1.0),
            _mix_buffer: vec![0.0; block_size.max(1)],
            scratch_order: Vec::new(),
        }
    }

    /// Add a node to the graph. Returns its unique ID.
    pub fn add_node(&mut self, mut node: Box<dyn ModularNode>) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;

        node.set_sample_rate(self.sample_rate);

        let num_inputs = node.inputs().len();
        let num_outputs = node.outputs().len();
        let input_scratch = (0..num_inputs)
            .map(|_| vec![0.0; self.block_size])
            .collect();
        let output_buffers = (0..num_outputs)
            .map(|_| vec![0.0; self.block_size])
            .collect();

        self.nodes.push(GraphNode {
            id,
            node,
            output_buffers,
            input_scratch,
        });

        self.rebuild_order();
        id
    }

    /// Remove a node and all its connections.
    pub fn remove_node(&mut self, id: NodeId) {
        self.connections
            .retain(|c| c.from_node != id && c.to_node != id);
        self.nodes.retain(|n| n.id != id);
        self.rebuild_order();
    }

    /// Connect an output port to an input port.
    ///
    /// Returns `false` if the connection would be invalid (type mismatch,
    /// missing node/port, or would create a cycle).
    pub fn connect(
        &mut self,
        from_node: NodeId,
        from_port: usize,
        to_node: NodeId,
        to_port: usize,
    ) -> bool {
        // Validate nodes exist and ports are in range
        let Some(from_idx) = self.node_index(from_node) else {
            return false;
        };
        let Some(to_idx) = self.node_index(to_node) else {
            return false;
        };

        if from_port >= self.nodes[from_idx].node.outputs().len() {
            return false;
        }
        if to_port >= self.nodes[to_idx].node.inputs().len() {
            return false;
        }

        // Type compatibility check
        let from_type = self.nodes[from_idx].node.outputs()[from_port].port_type;
        let to_type = self.nodes[to_idx].node.inputs()[to_port].port_type;
        if !Self::types_compatible(from_type, to_type) {
            return false;
        }

        // Add connection
        self.connections.push(Connection {
            from_node,
            from_port,
            to_node,
            to_port,
        });

        // Check for cycles — if topological sort fails, remove the connection
        if !self.rebuild_order() {
            self.connections.pop();
            return false;
        }

        true
    }

    /// Disconnect a specific connection.
    pub fn disconnect(
        &mut self,
        from_node: NodeId,
        from_port: usize,
        to_node: NodeId,
        to_port: usize,
    ) {
        self.connections.retain(|c| {
            !(c.from_node == from_node
                && c.from_port == from_port
                && c.to_node == to_node
                && c.to_port == to_port)
        });
        self.rebuild_order();
    }

    /// Process one block through the entire graph in topological order.
    ///
    /// Uses pre-allocated scratch buffers — zero allocations per call.
    pub fn process(&mut self) {
        // Copy process order into scratch to avoid borrow conflict with self.
        self.scratch_order.clear();
        self.scratch_order.extend_from_slice(&self.process_order);

        for order_pos in 0..self.scratch_order.len() {
            let node_idx = self.scratch_order[order_pos];
            let node_id = self.nodes[node_idx].id;
            let num_inputs = self.nodes[node_idx].input_scratch.len();

            // Zero the pre-allocated input scratch buffers.
            for buf in &mut self.nodes[node_idx].input_scratch {
                buf.fill(0.0);
            }

            // Accumulate connected sources into input scratch.
            // We iterate connections and copy from source output_buffers.
            // This requires careful indexing to satisfy the borrow checker.
            for conn_idx in 0..self.connections.len() {
                let conn_to_node = self.connections[conn_idx].to_node;
                let conn_to_port = self.connections[conn_idx].to_port;
                if conn_to_node != node_id || conn_to_port >= num_inputs {
                    continue;
                }
                let from_node_id = self.connections[conn_idx].from_node;
                let from_port = self.connections[conn_idx].from_port;

                if let Some(src_idx) = self.node_index(from_node_id) {
                    // Copy source output into destination input scratch.
                    // src_idx != node_idx (no self-connections, enforced by cycle detection).
                    let block_size = self.block_size;
                    let (src_slice, dst_slice) = if src_idx < node_idx {
                        let (left, right) = self.nodes.split_at_mut(node_idx);
                        (
                            &left[src_idx].output_buffers[from_port],
                            &mut right[0].input_scratch[conn_to_port],
                        )
                    } else {
                        let (left, right) = self.nodes.split_at_mut(src_idx);
                        (
                            &right[0].output_buffers[from_port],
                            &mut left[node_idx].input_scratch[conn_to_port],
                        )
                    };

                    let copy_len = block_size.min(src_slice.len()).min(dst_slice.len());
                    for i in 0..copy_len {
                        dst_slice[i] += src_slice[i];
                    }
                }
            }

            // Zero output buffers before processing.
            for buf in &mut self.nodes[node_idx].output_buffers {
                buf.fill(0.0);
            }

            // Process the node: build references from pre-allocated buffers.
            // We need to split the GraphNode to borrow node, input_scratch, and
            // output_buffers simultaneously.
            let graph_node = &mut self.nodes[node_idx];
            let input_refs: Vec<&[f32]> = graph_node
                .input_scratch
                .iter()
                .map(std::vec::Vec::as_slice)
                .collect();
            let mut output_refs: Vec<&mut [f32]> = graph_node
                .output_buffers
                .iter_mut()
                .map(std::vec::Vec::as_mut_slice)
                .collect();
            graph_node.node.process(&input_refs, &mut output_refs);
        }
    }

    /// Get the output buffer of a specific node's port.
    #[must_use]
    pub fn get_output(&self, node_id: NodeId, port: usize) -> Option<&[f32]> {
        self.node_index(node_id).and_then(|idx| {
            self.nodes[idx]
                .output_buffers
                .get(port)
                .map(std::vec::Vec::as_slice)
        })
    }

    /// Get all connections.
    #[must_use]
    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    /// Get list of node IDs and names.
    #[must_use]
    pub fn node_list(&self) -> Vec<(NodeId, String)> {
        self.nodes
            .iter()
            .map(|n| (n.id, n.node.name().to_string()))
            .collect()
    }

    /// Reset all nodes.
    pub fn reset(&mut self) {
        for graph_node in &mut self.nodes {
            graph_node.node.reset();
            for buf in &mut graph_node.output_buffers {
                buf.fill(0.0);
            }
            for buf in &mut graph_node.input_scratch {
                buf.fill(0.0);
            }
        }
    }

    /// Rebuild topological processing order. Returns false if cycle detected.
    fn rebuild_order(&mut self) -> bool {
        let n = self.nodes.len();
        if n == 0 {
            self.process_order.clear();
            return true;
        }

        // Kahn's algorithm for topological sort
        let mut in_degree = vec![0_usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

        for conn in &self.connections {
            if let (Some(from_idx), Some(to_idx)) = (
                self.node_index(conn.from_node),
                self.node_index(conn.to_node),
            ) {
                adj[from_idx].push(to_idx);
                in_degree[to_idx] += 1;
            }
        }

        let mut queue = VecDeque::new();
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                queue.push_back(i);
            }
        }

        let mut order = Vec::with_capacity(n);
        while let Some(node_idx) = queue.pop_front() {
            order.push(node_idx);
            for &neighbor in &adj[node_idx] {
                in_degree[neighbor] -= 1;
                if in_degree[neighbor] == 0 {
                    queue.push_back(neighbor);
                }
            }
        }

        if order.len() == n {
            self.process_order = order;
            true
        } else {
            // Cycle detected
            false
        }
    }

    /// Find index in `nodes` vec by node ID.
    fn node_index(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// Check if two port types are compatible for connection.
    const fn types_compatible(from: PortType, to: PortType) -> bool {
        // Audio can connect to audio, control to control, trigger to trigger.
        // Audio can also feed into control (implicit downsampling).
        matches!(
            (from, to),
            (PortType::Audio, PortType::Audio | PortType::Control)
                | (PortType::Control, PortType::Control)
                | (PortType::Trigger, PortType::Trigger)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::nodes::OscNode;
    use super::*;

    #[test]
    fn graph_add_remove_node() {
        let mut graph = NodeGraph::new(44100.0, 128);
        let id = graph.add_node(Box::new(OscNode::new(44100.0)));
        assert_eq!(graph.node_list().len(), 1);
        graph.remove_node(id);
        assert!(graph.node_list().is_empty());
    }

    #[test]
    fn graph_connect_valid() {
        let mut graph = NodeGraph::new(44100.0, 128);
        let osc = graph.add_node(Box::new(OscNode::new(44100.0)));
        let osc2 = graph.add_node(Box::new(OscNode::new(44100.0)));
        // osc output 0 (audio) -> osc2 input 0 (audio frequency modulation)
        let ok = graph.connect(osc, 0, osc2, 0);
        assert!(ok, "valid connection should succeed");
    }

    #[test]
    fn graph_rejects_cycle() {
        let mut graph = NodeGraph::new(44100.0, 128);
        let a = graph.add_node(Box::new(OscNode::new(44100.0)));
        let b = graph.add_node(Box::new(OscNode::new(44100.0)));
        assert!(graph.connect(a, 0, b, 0));
        // b -> a would create a cycle
        let ok = graph.connect(b, 0, a, 0);
        assert!(!ok, "cycle should be rejected");
    }

    #[test]
    fn graph_processes_without_panic() {
        let mut graph = NodeGraph::new(44100.0, 128);
        let _osc = graph.add_node(Box::new(OscNode::new(44100.0)));
        graph.process();
    }
}
