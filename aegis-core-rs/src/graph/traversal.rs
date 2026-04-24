use petgraph::Direction;
use crate::graph::dependency::DependencyGraph;

pub fn fan_out_violations(graph: &DependencyGraph, limit: usize) -> Vec<(String, usize)> {
    let mut violations = Vec::new();
    for idx in graph.graph.node_indices() {
        let count = graph.graph.neighbors_directed(idx, Direction::Outgoing).count();
        if count > limit {
            violations.push((graph.graph[idx].clone(), count));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fan_out_violation() {
        let mut g = DependencyGraph::new();
        g.build_from_edges(vec![
            ("main".into(), "a".into()),
            ("main".into(), "b".into()),
            ("main".into(), "c".into()),
        ]);
        let v = fan_out_violations(&g, 2);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "main");
        assert_eq!(v[0].1, 3);
    }

    #[test]
    fn test_no_violation_within_limit() {
        let mut g = DependencyGraph::new();
        g.build_from_edges(vec![("a".into(), "b".into())]);
        assert!(fan_out_violations(&g, 5).is_empty());
    }
}
