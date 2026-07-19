use dashmap::DashMap;
use uuid::Uuid;
use std::collections::VecDeque;
use std::sync::Arc;

pub const DEFAULT_MAX_DEPTH: u32 = 5;

#[derive(Debug, Clone)]
pub struct HierarchyNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub children: Vec<Uuid>,
    pub depth: u32,
}

#[derive(Debug, Clone)]
pub struct HierarchyTree {
    nodes: Arc<DashMap<Uuid, HierarchyNode>>,
    max_depth: u32,
}

impl HierarchyTree {
    pub fn new(max_depth: u32) -> Self {
        Self {
            nodes: Arc::new(DashMap::new()),
            max_depth,
        }
    }

    pub fn add_node(&self, id: Uuid, parent_id: Option<Uuid>) -> Result<(), HierarchyError> {
        if self.nodes.contains_key(&id) {
            return Err(HierarchyError::DuplicateNode(id));
        }

        if let Some(pid) = parent_id {
            if self.is_ancestor_of(id, pid) {
                return Err(HierarchyError::CycleDetected { child: id, parent: pid });
            }
        }

        let depth = match parent_id {
            Some(pid) => {
                let parent = self.nodes.get(&pid).ok_or(HierarchyError::ParentNotFound(pid))?;
                let child_depth = parent.depth + 1;
                if child_depth > self.max_depth {
                    return Err(HierarchyError::MaxDepthExceeded {
                        depth: child_depth,
                        max: self.max_depth,
                    });
                }
                child_depth
            }
            None => 0,
        };

        let node = HierarchyNode {
            id,
            parent_id,
            children: Vec::new(),
            depth,
        };

        if let Some(pid) = parent_id {
            if let Some(mut parent) = self.nodes.get_mut(&pid) {
                parent.children.push(id);
            }
        }

        self.nodes.insert(id, node);
        Ok(())
    }

    pub fn is_ancestor_of(&self, ancestor: Uuid, descendant: Uuid) -> bool {
        if ancestor == descendant {
            return true;
        }
        let mut current = self.get_parent(&descendant);
        while let Some(pid) = current {
            if pid == ancestor {
                return true;
            }
            current = self.get_parent(&pid);
        }
        false
    }

    pub fn remove_subtree(&self, node_id: &Uuid) -> Vec<Uuid> {
        let mut removed = Vec::new();
        let mut queue: VecDeque<Uuid> = VecDeque::new();
        queue.push_back(*node_id);

        while let Some(id) = queue.pop_front() {
            let children_snapshot = self.get_children(&id);
            queue.extend(children_snapshot.iter().copied());
            self.remove_node(&id);
            removed.push(id);
        }

        removed
    }

    pub fn remove_node(&self, id: &Uuid) {
        if let Some((_, node)) = self.nodes.remove(id) {
            if let Some(pid) = node.parent_id {
                if let Some(mut parent) = self.nodes.get_mut(&pid) {
                    parent.children.retain(|c| c != id);
                }
            }
        }
    }

    pub fn cleanup_orphans(&self) -> Vec<Uuid> {
        let root = self.root_id();
        let orphan_ids: Vec<Uuid> = self
            .nodes
            .iter()
            .filter(|n| {
                if let Some(pid) = n.parent_id {
                    !self.nodes.contains_key(&pid)
                } else {
                    false
                }
            })
            .map(|n| n.id)
            .collect();

        let mut cleaned = Vec::new();
        for oid in &orphan_ids {
            match root {
                Some(root_id) => {
                    if let Some(mut parent) = self.nodes.get_mut(&root_id) {
                        parent.children.push(*oid);
                        if let Some(mut orphan) = self.nodes.get_mut(oid) {
                            orphan.parent_id = Some(root_id);
                            orphan.depth = 1;
                        }
                    }
                    cleaned.push(*oid);
                }
                None => {
                    self.remove_node(oid);
                    cleaned.push(*oid);
                }
            }
        }

        cleaned
    }

    pub fn validate_dag(&self) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();

        if self.nodes.is_empty() {
            return Ok(());
        }

        let root_count = self.nodes.iter().filter(|n| n.parent_id.is_none()).count();
        if root_count == 0 {
            errors.push("DAG has no root node".into());
        }
        if root_count > 1 {
            errors.push(format!("DAG has multiple roots ({})", root_count));
        }

        for node in self.nodes.iter() {
            if let Some(pid) = node.parent_id {
                if !self.nodes.contains_key(&pid) {
                    errors.push(format!(
                        "node {} references missing parent {}",
                        node.id, pid
                    ));
                }
            }

            let expected_depth = self.compute_depth(node.id);
            if expected_depth != node.depth {
                errors.push(format!(
                    "node {} has depth {} but expected depth {}",
                    node.id, node.depth, expected_depth
                ));
            }

            if node.depth > self.max_depth {
                errors.push(format!(
                    "node {} depth {} exceeds max_depth {}",
                    node.id, node.depth, self.max_depth
                ));
            }

            for child_id in &node.children {
                if !self.nodes.contains_key(child_id) {
                    errors.push(format!(
                        "node {} lists missing child {}",
                        node.id, child_id
                    ));
                } else if let Some(child) = self.nodes.get(child_id) {
                    if child.parent_id != Some(node.id) {
                        errors.push(format!(
                            "node {} has child {} but child's parent is {:?}",
                            node.id, child_id, child.parent_id
                        ));
                    }
                }
            }
        }

        let cycle_nodes = self.detect_cycles();
        for cycle in &cycle_nodes {
            errors.push(format!("cycle detected involving node {}", cycle));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn get_children(&self, id: &Uuid) -> Vec<Uuid> {
        self.nodes.get(id).map(|n| n.children.clone()).unwrap_or_default()
    }

    pub fn get_parent(&self, id: &Uuid) -> Option<Uuid> {
        self.nodes.get(id).and_then(|n| n.parent_id)
    }

    pub fn get_depth(&self, id: &Uuid) -> Option<u32> {
        self.nodes.get(id).map(|n| n.depth)
    }

    pub fn root_id(&self) -> Option<Uuid> {
        self.nodes.iter().find(|n| n.parent_id.is_none()).map(|n| n.id)
    }

    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn compute_depth(&self, node_id: Uuid) -> u32 {
        let mut depth = 0u32;
        let mut current = self.get_parent(&node_id);
        while let Some(pid) = current {
            depth += 1;
            current = self.get_parent(&pid);
        }
        depth
    }

    fn detect_cycles(&self) -> Vec<Uuid> {
        let mut all_ids: Vec<Uuid> = self.nodes.iter().map(|n| n.id).collect();
        all_ids.sort_by_key(|id| id.as_u128());

        let mut in_degree: std::collections::HashMap<Uuid, usize> =
            std::collections::HashMap::new();
        let mut adjacency: std::collections::HashMap<Uuid, Vec<Uuid>> =
            std::collections::HashMap::new();

        for node in self.nodes.iter() {
            let id = node.id;
            in_degree.entry(id).or_insert(0);
            for child in &node.children {
                if self.nodes.contains_key(child) {
                    adjacency.entry(id).or_default().push(*child);
                    *in_degree.entry(*child).or_insert(0) += 1;
                }
            }
        }

        let mut queue: VecDeque<Uuid> = VecDeque::new();
        for (&id, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(id);
            }
        }

        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(neighbors) = adjacency.get(&id) {
                for nid in neighbors {
                    if let Some(deg) = in_degree.get_mut(nid) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(*nid);
                        }
                    }
                }
            }
        }

        if visited == all_ids.len() {
            return Vec::new();
        }

        let mut in_cycle: Vec<Uuid> =
            in_degree.iter().filter(|(_, d)| **d > 0).map(|(&id, _)| id).collect();
        in_cycle.sort_by_key(|id| id.as_u128());
        in_cycle
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HierarchyError {
    #[error("parent node {0} not found")]
    ParentNotFound(Uuid),

    #[error("max depth exceeded: {depth} > {max}")]
    MaxDepthExceeded { depth: u32, max: u32 },

    #[error("node {0} already exists")]
    DuplicateNode(Uuid),

    #[error("cycle detected: node {child} cannot be child of {parent} (would create a circular dependency)")]
    CycleDetected { child: Uuid, parent: Uuid },
}
