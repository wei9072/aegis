use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

pub struct DependencyGraph {
    pub(crate) graph: DiGraph<String, ()>,
    pub(crate) node_map: HashMap<String, NodeIndex>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self { graph: DiGraph::new(), node_map: HashMap::new() }
    }

    pub fn build_from_edges(&mut self, edges: Vec<(String, String)>) {
        for (src, tgt) in edges {
            let s = self.get_or_create(src);
            let t = self.get_or_create(tgt);
            self.graph.add_edge(s, t, ());
        }
    }

    pub fn nodes(&self) -> Vec<String> {
        self.node_map.keys().cloned().collect()
    }

    pub(crate) fn get_or_create(&mut self, name: String) -> NodeIndex {
        if let Some(&idx) = self.node_map.get(&name) {
            return idx;
        }
        let idx = self.graph.add_node(name.clone());
        self.node_map.insert(name, idx);
        idx
    }
}
