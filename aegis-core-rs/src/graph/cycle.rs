use petgraph::algo::is_cyclic_directed;
use crate::graph::dependency::DependencyGraph;

pub fn has_cycle(graph: &DependencyGraph) -> bool {
    is_cyclic_directed(&graph.graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_detected() {
        let mut g = DependencyGraph::new();
        g.build_from_edges(vec![
            ("A".into(), "B".into()),
            ("B".into(), "C".into()),
            ("C".into(), "A".into()),
        ]);
        assert!(has_cycle(&g));
    }

    #[test]
    fn test_no_cycle() {
        let mut g = DependencyGraph::new();
        g.build_from_edges(vec![("A".into(), "B".into()), ("B".into(), "C".into())]);
        assert!(!has_cycle(&g));
    }
}
