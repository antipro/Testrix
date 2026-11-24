use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A node in a package's execution DAG referencing an HTTP request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DagNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

impl PartialEq for DagNode {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.project_id == other.project_id
            && self.request_id == other.request_id
            && self.name == other.name
            && self.x.to_bits() == other.x.to_bits()
            && self.y.to_bits() == other.y.to_bits()
    }
}

impl Eq for DagNode {}

/// A directed dependency edge from `from` (upstream) to `to` (downstream).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// Directed Acyclic Graph (DAG) defining request execution order inside a Package (`RequestSet`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDag {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<DagNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<DagEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagValidationError {
    #[error("DAG node id must not be empty")]
    EmptyNodeId,
    #[error("duplicate DAG node id `{0}`")]
    DuplicateNodeId(String),
    #[error("DAG node `{node_id}` must specify a non-empty requestId")]
    EmptyRequestId { node_id: String },
    #[error("edge `{edge_from} -> {edge_to}` references unknown node `{missing}`")]
    MissingNodeReference {
        edge_from: String,
        edge_to: String,
        missing: String,
    },
    #[error("self-loop detected on node `{0}` (loops are not allowed in a DAG)")]
    SelfLoop(String),
    #[error("duplicate edge `{from} -> {to}`")]
    DuplicateEdge { from: String, to: String },
    #[error("cycle detected in DAG involving: {}", .0.join(" -> "))]
    CycleDetected(Vec<String>),
}

impl PackageDag {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Validates node uniqueness, edge references, and acyclicity (no loops).
    pub fn validate(&self) -> Result<(), DagValidationError> {
        let mut node_ids = BTreeSet::new();
        for node in &self.nodes {
            let id = node.id.trim();
            if id.is_empty() {
                return Err(DagValidationError::EmptyNodeId);
            }
            if !node_ids.insert(id.to_string()) {
                return Err(DagValidationError::DuplicateNodeId(id.to_string()));
            }
            if node.request_id.trim().is_empty() {
                return Err(DagValidationError::EmptyRequestId {
                    node_id: id.to_string(),
                });
            }
        }

        let mut seen_edges = BTreeSet::new();
        for edge in &self.edges {
            let from = edge.from.trim();
            let to = edge.to.trim();
            if !node_ids.contains(from) {
                return Err(DagValidationError::MissingNodeReference {
                    edge_from: from.to_string(),
                    edge_to: to.to_string(),
                    missing: from.to_string(),
                });
            }
            if !node_ids.contains(to) {
                return Err(DagValidationError::MissingNodeReference {
                    edge_from: from.to_string(),
                    edge_to: to.to_string(),
                    missing: to.to_string(),
                });
            }
            if from == to {
                return Err(DagValidationError::SelfLoop(from.to_string()));
            }
            if !seen_edges.insert((from.to_string(), to.to_string())) {
                return Err(DagValidationError::DuplicateEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                });
            }
        }

        let _ = self.topological_layers()?;
        Ok(())
    }

    /// Returns `true` if adding `from -> to` would create a self-loop, duplicate edge, or cycle.
    pub fn would_create_cycle(&self, from: &str, to: &str) -> bool {
        if from == to {
            return true;
        }
        if self.edges.iter().any(|e| e.from == from && e.to == to) {
            return true;
        }
        // Check if `from` is reachable from `to` via existing edges.
        let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            adj.entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(to);
        while let Some(curr) = queue.pop_front() {
            if curr == from {
                return true;
            }
            if visited.insert(curr)
                && let Some(nexts) = adj.get(curr)
            {
                for &next in nexts {
                    if !visited.contains(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        false
    }

    /// Computes topological execution layers using Kahn's algorithm.
    ///
    /// Returns `Err(DagValidationError::CycleDetected(_))` if any loop exists.
    pub fn topological_layers(&self) -> Result<Vec<Vec<String>>, DagValidationError> {
        let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
        let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for node in &self.nodes {
            let id = node.id.trim().to_string();
            in_degree.entry(id.clone()).or_insert(0);
            adj.entry(id).or_default();
        }
        for edge in &self.edges {
            let from = edge.from.trim().to_string();
            let to = edge.to.trim().to_string();
            if let Some(deg) = in_degree.get_mut(&to) {
                *deg += 1;
            }
            if let Some(neighbors) = adj.get_mut(&from) {
                neighbors.push(to);
            }
        }

        let mut current_layer: Vec<String> = in_degree
            .iter()
            .filter_map(|(id, &deg)| if deg == 0 { Some(id.clone()) } else { None })
            .collect();
        let mut layers = Vec::new();
        let mut visited_count = 0usize;

        while !current_layer.is_empty() {
            current_layer.sort();
            visited_count += current_layer.len();
            let mut next_layer = Vec::new();
            for id in &current_layer {
                if let Some(neighbors) = adj.get(id) {
                    for next in neighbors {
                        if let Some(deg) = in_degree.get_mut(next) {
                            *deg -= 1;
                            if *deg == 0 {
                                next_layer.push(next.clone());
                            }
                        }
                    }
                }
            }
            layers.push(current_layer);
            current_layer = next_layer;
        }

        if visited_count != self.nodes.len() {
            let remaining: Vec<String> = in_degree
                .into_iter()
                .filter_map(|(id, deg)| if deg > 0 { Some(id) } else { None })
                .collect();
            return Err(DagValidationError::CycleDetected(remaining));
        }

        Ok(layers)
    }

    /// Returns a flattened topological execution order of node IDs.
    pub fn topological_order(&self) -> Result<Vec<String>, DagValidationError> {
        let layers = self.topological_layers()?;
        Ok(layers.into_iter().flatten().collect())
    }

    /// Assigns clean left-to-right layered coordinates to nodes if all nodes are at `(0, 0)`.
    pub fn auto_layout_if_unpositioned(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let all_zero = self
            .nodes
            .iter()
            .all(|n| n.x.abs() < f32::EPSILON && n.y.abs() < f32::EPSILON);
        if !all_zero {
            return;
        }
        if let Ok(layers) = self.topological_layers() {
            for (col, layer) in layers.iter().enumerate() {
                for (row, node_id) in layer.iter().enumerate() {
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == *node_id) {
                        node.x = 60.0 + (col as f32) * 260.0;
                        node.y = 60.0 + (row as f32) * 110.0;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, req: &str) -> DagNode {
        DagNode {
            id: id.to_string(),
            project_id: "proj-1".to_string(),
            request_id: req.to_string(),
            name: id.to_string(),
            x: 0.0,
            y: 0.0,
        }
    }

    fn edge(from: &str, to: &str) -> DagEdge {
        DagEdge {
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    #[test]
    fn validates_and_sorts_diamond_dag() {
        let dag = PackageDag {
            nodes: vec![
                node("login", "req-1"),
                node("profile", "req-2"),
                node("settings", "req-3"),
                node("logout", "req-4"),
            ],
            edges: vec![
                edge("login", "profile"),
                edge("login", "settings"),
                edge("profile", "logout"),
                edge("settings", "logout"),
            ],
        };
        assert!(dag.validate().is_ok());
        let layers = dag.topological_layers().unwrap();
        assert_eq!(
            layers,
            vec![
                vec!["login".to_string()],
                vec!["profile".to_string(), "settings".to_string()],
                vec!["logout".to_string()],
            ]
        );
    }

    #[test]
    fn rejects_cycle_and_self_loop() {
        let self_loop = PackageDag {
            nodes: vec![node("a", "req-1")],
            edges: vec![edge("a", "a")],
        };
        assert!(matches!(
            self_loop.validate(),
            Err(DagValidationError::SelfLoop(_))
        ));

        let cycle = PackageDag {
            nodes: vec![node("a", "req-1"), node("b", "req-2"), node("c", "req-3")],
            edges: vec![edge("a", "b"), edge("b", "c"), edge("c", "a")],
        };
        assert!(matches!(
            cycle.validate(),
            Err(DagValidationError::CycleDetected(_))
        ));
        assert!(cycle.would_create_cycle("c", "a"));
    }

    #[test]
    fn would_create_cycle_detects_transitive_back_edge() {
        let dag = PackageDag {
            nodes: vec![node("a", "req-1"), node("b", "req-2"), node("c", "req-3")],
            edges: vec![edge("a", "b"), edge("b", "c")],
        };
        assert!(dag.would_create_cycle("c", "a"));
        assert!(dag.would_create_cycle("b", "a"));
        assert!(!dag.would_create_cycle("a", "c"));
    }
}
