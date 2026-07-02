//! Audio graph model, compilation, scheduling, and latency compensation.
//!
//! The compile-then-execute seam from `docs/04` §3: a [`Graph`] of [`Node`]s
//! is compiled into a [`CompiledGraph`] — a topologically ordered task list
//! with **liveness-analyzed buffer assignment**, so a deep graph reuses a
//! small buffer pool instead of allocating one buffer per edge. Execution is
//! block-based and allocation-free after compilation, which is exactly the
//! shape the future real-time thread requires (`docs/10` §4).
//!
//! Deliberately deferred: latency compensation (PDC) and parallel level
//! execution — the schedule already records dependency order to make both
//! possible without changing this API.

/// Frames per processing block.
pub const BLOCK: usize = 512;

/// One stereo block of audio (fixed size, non-interleaved).
#[derive(Debug, Clone)]
pub struct StereoBlock {
    /// Left channel.
    pub left: [f32; BLOCK],
    /// Right channel.
    pub right: [f32; BLOCK],
}

impl StereoBlock {
    /// A silent block.
    pub fn silence() -> StereoBlock {
        StereoBlock {
            left: [0.0; BLOCK],
            right: [0.0; BLOCK],
        }
    }

    /// Zeroes the block in place.
    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
    }
}

/// A processing node. Implementations live outside this crate (the renderer,
/// instruments, future plugin wrappers) — the graph is engine-agnostic.
pub trait Node: Send {
    /// Produces one block starting at absolute `frame_offset`, reading zero or
    /// more input blocks. Must not allocate; called once per block in schedule
    /// order.
    fn process(&mut self, frame_offset: usize, inputs: &[&StereoBlock], output: &mut StereoBlock);
}

/// Identifies a node within one [`Graph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

/// A directed acyclic graph of processing nodes under construction.
#[derive(Default)]
pub struct Graph {
    nodes: Vec<Box<dyn Node>>,
    /// edges[i] = inputs of node i, in connection order.
    inputs: Vec<Vec<usize>>,
}

impl Graph {
    /// An empty graph.
    pub fn new() -> Graph {
        Graph::default()
    }

    /// Adds a node and returns its id.
    pub fn add(&mut self, node: Box<dyn Node>) -> NodeId {
        self.nodes.push(node);
        self.inputs.push(Vec::new());
        NodeId(self.nodes.len() - 1)
    }

    /// Connects `from`'s output into `to`'s inputs.
    ///
    /// # Errors
    /// Returns [`GraphError::UnknownNode`] for invalid ids.
    pub fn connect(&mut self, from: NodeId, to: NodeId) -> Result<(), GraphError> {
        if from.0 >= self.nodes.len() || to.0 >= self.nodes.len() {
            return Err(GraphError::UnknownNode);
        }
        self.inputs[to.0].push(from.0);
        Ok(())
    }

    /// Compiles the graph: topological sort (cycle detection), then buffer
    /// assignment by liveness — a buffer is recycled after its last consumer.
    ///
    /// # Errors
    /// Returns [`GraphError::Cycle`] if the graph is not a DAG, or
    /// [`GraphError::UnknownNode`] if `sink` is invalid.
    pub fn compile(self, sink: NodeId) -> Result<CompiledGraph, GraphError> {
        let n = self.nodes.len();
        if sink.0 >= n {
            return Err(GraphError::UnknownNode);
        }

        // Kahn topological sort over the whole graph.
        let mut indegree = vec![0usize; n];
        for (consumer, inputs) in self.inputs.iter().enumerate() {
            indegree[consumer] = inputs.len();
        }
        let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (consumer, inputs) in self.inputs.iter().enumerate() {
            for &producer in inputs {
                consumers[producer].push(consumer);
            }
        }
        let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(node) = ready.pop() {
            order.push(node);
            for &c in &consumers[node] {
                indegree[c] -= 1;
                if indegree[c] == 0 {
                    ready.push(c);
                }
            }
        }
        if order.len() != n {
            return Err(GraphError::Cycle);
        }

        // Liveness: last position in `order` where each node's output is read.
        let position: Vec<usize> = {
            let mut pos = vec![0usize; n];
            for (i, &node) in order.iter().enumerate() {
                pos[node] = i;
            }
            pos
        };
        let mut last_use = vec![0usize; n];
        for (consumer, inputs) in self.inputs.iter().enumerate() {
            for &producer in inputs {
                last_use[producer] = last_use[producer].max(position[consumer]);
            }
        }
        last_use[sink.0] = usize::MAX; // sink's buffer lives to the end

        // Assign buffers, recycling ones whose producer has no later readers.
        let mut free: Vec<usize> = Vec::new();
        let mut pool_size = 0usize;
        let mut buffer_of = vec![usize::MAX; n];
        let mut tasks = Vec::with_capacity(n);
        for (step, &node) in order.iter().enumerate() {
            let out = free.pop().unwrap_or_else(|| {
                pool_size += 1;
                pool_size - 1
            });
            buffer_of[node] = out;
            tasks.push(Task {
                node,
                input_buffers: self.inputs[node].iter().map(|&p| buffer_of[p]).collect(),
                output_buffer: out,
            });
            // Release input buffers whose last consumer just ran.
            for &producer in &self.inputs[node] {
                if last_use[producer] == step {
                    free.push(buffer_of[producer]);
                }
            }
        }

        Ok(CompiledGraph {
            nodes: self.nodes,
            tasks,
            buffers: (0..pool_size).map(|_| StereoBlock::silence()).collect(),
            sink_buffer: buffer_of[sink.0],
        })
    }
}

struct Task {
    node: usize,
    input_buffers: Vec<usize>,
    output_buffer: usize,
}

/// An executable schedule. Owns its nodes and a pre-allocated buffer pool;
/// [`CompiledGraph::process_block`] performs no allocation.
pub struct CompiledGraph {
    nodes: Vec<Box<dyn Node>>,
    tasks: Vec<Task>,
    buffers: Vec<StereoBlock>,
    sink_buffer: usize,
}

impl CompiledGraph {
    /// Number of buffers in the pool (exposed for tests/benchmarks).
    pub fn buffer_pool_size(&self) -> usize {
        self.buffers.len()
    }

    /// Processes one block at `frame_offset`, returning the sink's output.
    pub fn process_block(&mut self, frame_offset: usize) -> &StereoBlock {
        for task in &self.tasks {
            // Split-borrow dance: take the output buffer out, process, put back.
            let mut out = std::mem::replace(
                &mut self.buffers[task.output_buffer],
                StereoBlock::silence(),
            );
            {
                let inputs: Vec<&StereoBlock> = task
                    .input_buffers
                    .iter()
                    .map(|&i| &self.buffers[i])
                    .collect();
                self.nodes[task.node].process(frame_offset, &inputs, &mut out);
            }
            self.buffers[task.output_buffer] = out;
        }
        &self.buffers[self.sink_buffer]
    }

    /// Renders `total_frames` by repeated block processing into an owned
    /// non-interleaved stereo pair.
    pub fn render(&mut self, total_frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut left = Vec::with_capacity(total_frames);
        let mut right = Vec::with_capacity(total_frames);
        let mut offset = 0;
        while offset < total_frames {
            let take = BLOCK.min(total_frames - offset);
            let block = self.process_block(offset);
            left.extend_from_slice(&block.left[..take]);
            right.extend_from_slice(&block.right[..take]);
            offset += take;
        }
        (left, right)
    }
}

/// Errors from graph construction/compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// A node id did not belong to this graph.
    #[error("unknown node id")]
    UnknownNode,
    /// The graph contains a cycle.
    #[error("graph contains a cycle")]
    Cycle,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emits a constant value on both channels.
    struct Constant(f32);
    impl Node for Constant {
        fn process(&mut self, _: usize, _: &[&StereoBlock], out: &mut StereoBlock) {
            out.left.fill(self.0);
            out.right.fill(self.0);
        }
    }

    /// Sums all inputs.
    struct Sum;
    impl Node for Sum {
        fn process(&mut self, _: usize, inputs: &[&StereoBlock], out: &mut StereoBlock) {
            out.clear();
            for input in inputs {
                for (o, i) in out.left.iter_mut().zip(&input.left) {
                    *o += i;
                }
                for (o, i) in out.right.iter_mut().zip(&input.right) {
                    *o += i;
                }
            }
        }
    }

    /// Multiplies its single input by a factor.
    struct Gain(f32);
    impl Node for Gain {
        fn process(&mut self, _: usize, inputs: &[&StereoBlock], out: &mut StereoBlock) {
            for (o, i) in out.left.iter_mut().zip(&inputs[0].left) {
                *o = i * self.0;
            }
            for (o, i) in out.right.iter_mut().zip(&inputs[0].right) {
                *o = i * self.0;
            }
        }
    }

    #[test]
    fn sums_parallel_sources() {
        let mut g = Graph::new();
        let a = g.add(Box::new(Constant(0.25)));
        let b = g.add(Box::new(Constant(0.5)));
        let sum = g.add(Box::new(Sum));
        g.connect(a, sum).unwrap();
        g.connect(b, sum).unwrap();
        let mut compiled = g.compile(sum).unwrap();
        let block = compiled.process_block(0);
        assert!((block.left[0] - 0.75).abs() < 1e-6);
        assert!((block.right[BLOCK - 1] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn chains_reuse_buffers() {
        // a -> g1 -> g2 -> g3 -> g4: five nodes, but liveness needs only 2 buffers.
        let mut g = Graph::new();
        let a = g.add(Box::new(Constant(1.0)));
        let mut prev = a;
        for _ in 0..4 {
            let gain = g.add(Box::new(Gain(0.5)));
            g.connect(prev, gain).unwrap();
            prev = gain;
        }
        let mut compiled = g.compile(prev).unwrap();
        assert_eq!(
            compiled.buffer_pool_size(),
            2,
            "liveness must recycle chain buffers"
        );
        let block = compiled.process_block(0);
        assert!((block.left[0] - 0.0625).abs() < 1e-6); // 1.0 * 0.5^4
    }

    #[test]
    fn cycles_are_rejected() {
        let mut g = Graph::new();
        let a = g.add(Box::new(Gain(1.0)));
        let b = g.add(Box::new(Gain(1.0)));
        g.connect(a, b).unwrap();
        g.connect(b, a).unwrap();
        match g.compile(a) {
            Err(e) => assert_eq!(e, GraphError::Cycle),
            Ok(_) => panic!("cycle must be rejected"),
        }
    }

    #[test]
    fn render_concatenates_blocks_to_exact_length() {
        let mut g = Graph::new();
        let a = g.add(Box::new(Constant(0.1)));
        let mut compiled = g.compile(a).unwrap();
        let (l, r) = compiled.render(BLOCK + 37);
        assert_eq!(l.len(), BLOCK + 37);
        assert_eq!(r.len(), BLOCK + 37);
        assert!((l[BLOCK + 36] - 0.1).abs() < 1e-6);
    }
}
