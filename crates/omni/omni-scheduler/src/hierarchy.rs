//! Ajan hiyerarsisi ve spawn tavanlari (AS3, MASTER-PLAN 7.3).
//!
//! `omni-scheduler` spawn **aninda** zorlar:
//! - **Derinlik tavani:** varsayilan [`DEFAULT_MAX_DEPTH`] (`scheduler.max_depth`).
//!   `agents.depth` kolonundan okunur ([`HierarchyTree::restore_node`]); asildi ->
//!   spawn reddi + `Notice` ([`SpawnDenial`]).
//! - **Seviye basina fan-out tavani:** varsayilan [`DEFAULT_MAX_FANOUT`]
//!   (`scheduler.max_fanout`).
//! - **Global aktif tavani:** burada **yoktur** — kaynak valisi (7.2,
//!   `admission.rs`) belirler; sabit degildir.
//!
//! I5/AS8: tavanlar koda gomulu sabit degil, [`SpawnLimits`] ile disaridan
//! verilir; `KEY_*` anahtarlari `omni-config` `ConfigStore`'dan okunur.
//!
//! I6: uretim yolunda `unwrap`/`expect`/`panic!` yok.

use std::collections::VecDeque;
use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

/// Derinlik tavani yapilandirma anahtari (7.3).
pub const KEY_MAX_DEPTH: &str = "scheduler.max_depth";
/// Fan-out tavani yapilandirma anahtari (7.3).
pub const KEY_MAX_FANOUT: &str = "scheduler.max_fanout";

/// Derinlik tavani varsayilani (AS3).
pub const DEFAULT_MAX_DEPTH: u32 = 5;
/// Seviye basina fan-out tavani varsayilani (AS3).
pub const DEFAULT_MAX_FANOUT: u32 = 8;

/// Spawn tavanlari (7.3). Global aktif tavani **burada yok**; o kaynak valisinin
/// isidir (7.2) ve dinamiktir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnLimits {
    /// Kok `depth = 0` kabul edilir; bu degeri **asan** derinlik reddedilir.
    pub max_depth: u32,
    /// Bir ebeveynin dogrudan cocuk sayisi tavani.
    pub max_fanout: u32,
}

impl SpawnLimits {
    /// Yapilandirmadan okunmus degerlerle tavan kumesi. `None` -> varsayilan.
    #[must_use]
    pub fn from_config(max_depth: Option<u32>, max_fanout: Option<u32>) -> Self {
        Self {
            max_depth: max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
            max_fanout: max_fanout.unwrap_or(DEFAULT_MAX_FANOUT),
        }
    }

    /// Persona `max_depth` override'i (11.1) uygulanmis kopya.
    ///
    /// Override yalnizca **daraltabilir**; bir persona sistem tavanini asamaz.
    #[must_use]
    pub fn with_depth_override(self, override_depth: Option<u32>) -> Self {
        match override_depth {
            Some(d) => Self { max_depth: self.max_depth.min(d), ..self },
            None => self,
        }
    }
}

impl Default for SpawnLimits {
    fn default() -> Self {
        Self { max_depth: DEFAULT_MAX_DEPTH, max_fanout: DEFAULT_MAX_FANOUT }
    }
}

/// Spawn reddi bildirimi (7.3 "spawn reddi + `Notice`").
///
/// Bu crate `omni-proto`'ya bagli degildir; cagiran taraf bunu `NoticeView::new(
/// NoticeLevel::Warn, denial.code, denial.message, ts)` ile akisa koyar (I3:
/// tek durum kaynagi yine `omni-proto` kalir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnDenial {
    /// `NoticeView.code` ile birebir eslesen makine-okunur kod.
    pub code: &'static str,
    /// `NoticeView.message` — insan okuyacagi metin.
    pub message: String,
    /// Spawn'i isteyen ebeveyn ajan, varsa.
    pub parent_id: Option<Uuid>,
    /// Reddedilen aday ajan kimligi.
    pub agent_id: Uuid,
}

/// Hiyerarsideki tek dugum. `depth` alani `agents.depth` kolonuna karsilik gelir.
#[derive(Debug, Clone)]
pub struct HierarchyNode {
    /// Ajan kimligi (`agents.id`).
    pub id: Uuid,
    /// Ebeveyn ajan (`agents.parent_agent_id`).
    pub parent_id: Option<Uuid>,
    /// Dogrudan cocuklar.
    pub children: Vec<Uuid>,
    /// Kokten uzaklik (`agents.depth`).
    pub depth: u32,
}

/// Ajan hiyerarsisi; spawn tavanlarini zorlayan tek nokta (7.3).
#[derive(Debug, Clone)]
pub struct HierarchyTree {
    nodes: Arc<DashMap<Uuid, HierarchyNode>>,
    limits: SpawnLimits,
}

impl HierarchyTree {
    /// Yalnizca derinlik tavani verilerek kurar; fan-out varsayilana duser.
    #[must_use]
    pub fn new(max_depth: u32) -> Self {
        Self::with_limits(SpawnLimits { max_depth, max_fanout: DEFAULT_MAX_FANOUT })
    }

    /// Tam tavan kumesiyle kurar (7.3).
    #[must_use]
    pub fn with_limits(limits: SpawnLimits) -> Self {
        Self { nodes: Arc::new(DashMap::new()), limits }
    }

    /// **Spawn kapisi (AS3/7.3).** Yan etkisiz on-kontrol: derinlik ve fan-out
    /// tavanlarini dener, gecerse cocugun alacagi derinligi doner.
    ///
    /// `depth_override` persona semasindaki `max_depth` (11.1); yalnizca daraltir.
    pub fn check_spawn(
        &self,
        parent_id: Option<Uuid>,
        depth_override: Option<u32>,
    ) -> Result<u32, HierarchyError> {
        let limits = self.limits.with_depth_override(depth_override);
        let Some(pid) = parent_id else {
            // Kok spawn: derinlik 0. Kok icin fan-out kavrami yoktur.
            return Ok(0);
        };
        let parent = self.nodes.get(&pid).ok_or(HierarchyError::ParentNotFound(pid))?;
        let child_depth = parent.depth.saturating_add(1);
        if child_depth > limits.max_depth {
            return Err(HierarchyError::MaxDepthExceeded {
                depth: child_depth,
                max: limits.max_depth,
            });
        }
        let fanout = parent.children.len() as u32;
        if fanout >= limits.max_fanout {
            return Err(HierarchyError::MaxFanoutExceeded {
                parent: pid,
                fanout: fanout.saturating_add(1),
                max: limits.max_fanout,
            });
        }
        Ok(child_depth)
    }

    /// Dugumu ekler; tavanlar [`Self::check_spawn`] ile **eklemeden once** zorlanir.
    pub fn add_node(&self, id: Uuid, parent_id: Option<Uuid>) -> Result<(), HierarchyError> {
        self.add_node_with_override(id, parent_id, None)
    }

    /// Persona `max_depth` override'i (11.1) ile dugum ekler.
    pub fn add_node_with_override(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        depth_override: Option<u32>,
    ) -> Result<(), HierarchyError> {
        if self.nodes.contains_key(&id) {
            return Err(HierarchyError::DuplicateNode(id));
        }

        if let Some(pid) = parent_id
            && self.is_ancestor_of(id, pid)
        {
            return Err(HierarchyError::CycleDetected { child: id, parent: pid });
        }

        let depth = self.check_spawn(parent_id, depth_override)?;

        let node = HierarchyNode { id, parent_id, children: Vec::new(), depth };

        if let Some(pid) = parent_id
            && let Some(mut parent) = self.nodes.get_mut(&pid)
        {
            parent.children.push(id);
        }

        self.nodes.insert(id, node);
        Ok(())
    }

    /// Crash-only yeniden kurulum (3.2): `agents` satirindan (`id`,
    /// `parent_agent_id`, `depth`) dugumu **tavan kontrolu yapmadan** geri koyar.
    ///
    /// Zaten var olmus bir agac replay edilirken tavan reddi uretmek yanlis
    /// olur: tavan spawn aninda zorlanmistir. Tutarsizlik [`Self::validate_dag`]
    /// ile raporlanir.
    pub fn restore_node(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        depth: u32,
    ) -> Result<(), HierarchyError> {
        if self.nodes.contains_key(&id) {
            return Err(HierarchyError::DuplicateNode(id));
        }
        self.nodes.insert(id, HierarchyNode { id, parent_id, children: Vec::new(), depth });
        if let Some(pid) = parent_id
            && let Some(mut parent) = self.nodes.get_mut(&pid)
            && !parent.children.contains(&id)
        {
            parent.children.push(id);
        }
        Ok(())
    }

    /// Yururlukteki tavanlar.
    #[must_use]
    pub fn limits(&self) -> SpawnLimits {
        self.limits
    }

    /// Bir ebeveynin dogrudan cocuk sayisi (fan-out).
    #[must_use]
    pub fn fanout_of(&self, parent_id: &Uuid) -> u32 {
        self.nodes.get(parent_id).map(|n| n.children.len() as u32).unwrap_or(0)
    }

    /// `ancestor`, `descendant`'in atasi mi? (kendisi de sayilir)
    #[must_use]
    pub fn is_ancestor_of(&self, ancestor: Uuid, descendant: Uuid) -> bool {
        if ancestor == descendant {
            return true;
        }
        let mut current = self.get_parent(&descendant);
        let mut guard = 0usize;
        let cap = self.nodes.len().saturating_add(1);
        while let Some(pid) = current {
            guard += 1;
            if guard > cap {
                // Bozuk agac; sessiz sonsuz dongu yerine "ata degil" de.
                return false;
            }
            if pid == ancestor {
                return true;
            }
            current = self.get_parent(&pid);
        }
        false
    }

    /// Alt agaci siler; silinen kimlikleri doner.
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

    /// Tek dugumu siler ve ebeveyninin cocuk listesinden dusurur.
    pub fn remove_node(&self, id: &Uuid) {
        if let Some((_, node)) = self.nodes.remove(id)
            && let Some(pid) = node.parent_id
            && let Some(mut parent) = self.nodes.get_mut(&pid)
        {
            parent.children.retain(|c| c != id);
        }
    }

    /// Ebeveyni kaybolmus dugumleri koke baglar; kok yoksa siler.
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

    /// Agacin butunlugunu dogrular; tavan ihlalleri de rapor edilir.
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
            if let Some(pid) = node.parent_id
                && !self.nodes.contains_key(&pid)
            {
                errors.push(format!("node {} references missing parent {}", node.id, pid));
            }

            let expected_depth = self.compute_depth(node.id);
            if expected_depth != node.depth {
                errors.push(format!(
                    "node {} has depth {} but expected depth {}",
                    node.id, node.depth, expected_depth
                ));
            }

            if node.depth > self.limits.max_depth {
                errors.push(format!(
                    "node {} depth {} exceeds max_depth {}",
                    node.id, node.depth, self.limits.max_depth
                ));
            }

            if node.children.len() as u32 > self.limits.max_fanout {
                errors.push(format!(
                    "node {} fanout {} exceeds max_fanout {}",
                    node.id,
                    node.children.len(),
                    self.limits.max_fanout
                ));
            }

            for child_id in &node.children {
                if !self.nodes.contains_key(child_id) {
                    errors.push(format!("node {} lists missing child {}", node.id, child_id));
                } else if let Some(child) = self.nodes.get(child_id)
                    && child.parent_id != Some(node.id)
                {
                    errors.push(format!(
                        "node {} has child {} but child's parent is {:?}",
                        node.id, child_id, child.parent_id
                    ));
                }
            }
        }

        let cycle_nodes = self.detect_cycles();
        for cycle in &cycle_nodes {
            errors.push(format!("cycle detected involving node {}", cycle));
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    /// Dogrudan cocuklar.
    #[must_use]
    pub fn get_children(&self, id: &Uuid) -> Vec<Uuid> {
        self.nodes.get(id).map(|n| n.children.clone()).unwrap_or_default()
    }

    /// Ebeveyn kimligi.
    #[must_use]
    pub fn get_parent(&self, id: &Uuid) -> Option<Uuid> {
        self.nodes.get(id).and_then(|n| n.parent_id)
    }

    /// `agents.depth` degeri.
    #[must_use]
    pub fn get_depth(&self, id: &Uuid) -> Option<u32> {
        self.nodes.get(id).map(|n| n.depth)
    }

    /// Koke kadar ata zinciri (kok en sonda).
    #[must_use]
    pub fn ancestors_of(&self, id: &Uuid) -> Vec<Uuid> {
        let mut chain = Vec::new();
        let cap = self.nodes.len();
        let mut current = self.get_parent(id);
        while let Some(pid) = current {
            if chain.len() >= cap {
                break;
            }
            chain.push(pid);
            current = self.get_parent(&pid);
        }
        chain
    }

    /// Kok dugum kimligi.
    #[must_use]
    pub fn root_id(&self) -> Option<Uuid> {
        self.nodes.iter().find(|n| n.parent_id.is_none()).map(|n| n.id)
    }

    /// Derinlik tavani.
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.limits.max_depth
    }

    /// Fan-out tavani.
    #[must_use]
    pub fn max_fanout(&self) -> u32 {
        self.limits.max_fanout
    }

    /// Dugum sayisi.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Agac bos mu?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn compute_depth(&self, node_id: Uuid) -> u32 {
        let mut depth = 0u32;
        let cap = self.nodes.len() as u32;
        let mut current = self.get_parent(&node_id);
        while let Some(pid) = current {
            if depth >= cap {
                break;
            }
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

/// Hiyerarsi/spawn tavani hatalari (7.3).
#[derive(Debug, thiserror::Error)]
pub enum HierarchyError {
    /// Ebeveyn dugum yok.
    #[error("parent node {0} not found")]
    ParentNotFound(Uuid),

    /// Derinlik tavani asildi (AS3).
    ///
    /// Alan kumesi bilerek `{ depth, max }` olarak korunur; mevcut tuketiciler
    /// bu varyanti alan-alan cozuyor.
    #[error("max depth exceeded: {depth} > {max}")]
    MaxDepthExceeded {
        /// Cocugun alacagi derinlik.
        depth: u32,
        /// Yururlukteki tavan.
        max: u32,
    },

    /// Seviye basina fan-out tavani asildi (AS3).
    #[error("fan-out tavani asildi: {parent} icin {fanout} > {max}")]
    MaxFanoutExceeded {
        /// Spawn'i isteyen ebeveyn.
        parent: Uuid,
        /// Bu spawn ile olusacak cocuk sayisi.
        fanout: u32,
        /// Yururlukteki tavan.
        max: u32,
    },

    /// Dugum zaten var.
    #[error("node {0} already exists")]
    DuplicateNode(Uuid),

    /// Dongu.
    #[error(
        "cycle detected: node {child} cannot be child of {parent} (would create a circular dependency)"
    )]
    CycleDetected {
        /// Aday cocuk.
        child: Uuid,
        /// Aday ebeveyn.
        parent: Uuid,
    },
}

impl HierarchyError {
    /// `NoticeView.code` ile birebir eslesen makine-okunur kod.
    #[must_use]
    pub fn notice_code(&self) -> &'static str {
        match self {
            Self::ParentNotFound(_) => "spawn_parent_missing",
            Self::MaxDepthExceeded { .. } => "depth_cap",
            Self::MaxFanoutExceeded { .. } => "fanout_cap",
            Self::DuplicateNode(_) => "spawn_duplicate_agent",
            Self::CycleDetected { .. } => "spawn_cycle",
        }
    }

    /// Bu hatanin spawn reddi olarak bildirilecek hali (7.3 "spawn reddi + Notice").
    #[must_use]
    pub fn to_denial(&self, agent_id: Uuid) -> SpawnDenial {
        let parent_id = match self {
            Self::ParentNotFound(pid) => Some(*pid),
            Self::MaxFanoutExceeded { parent, .. } | Self::CycleDetected { parent, .. } => {
                Some(*parent)
            }
            // Derinlik varyanti ebeveyn tasimaz (alan kumesi korundu).
            Self::MaxDepthExceeded { .. } | Self::DuplicateNode(_) => None,
        };
        SpawnDenial {
            code: self.notice_code(),
            message: self.to_string(),
            parent_id,
            agent_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `check_spawn`/`add_node` hatasini test icinde okunur hale getirir.
    fn err_of<T>(r: Result<T, HierarchyError>) -> Option<HierarchyError> {
        r.err()
    }

    #[test]
    fn defaults_match_as3() {
        let limits = SpawnLimits::default();
        assert_eq!(limits.max_depth, 5);
        assert_eq!(limits.max_fanout, 8);
    }

    #[test]
    fn config_falls_back_to_defaults() {
        let limits = SpawnLimits::from_config(None, None);
        assert_eq!(limits, SpawnLimits::default());
        let limits = SpawnLimits::from_config(Some(3), Some(2));
        assert_eq!(limits.max_depth, 3);
        assert_eq!(limits.max_fanout, 2);
    }

    #[test]
    fn depth_cap_denies_spawn() {
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 2, max_fanout: 8 });
        let mut chain = Vec::new();
        let root = Uuid::new_v4();
        assert!(tree.add_node(root, None).is_ok());
        chain.push(root);
        for _ in 0..2 {
            let id = Uuid::new_v4();
            let parent = chain.last().copied();
            assert!(tree.add_node(id, parent).is_ok());
            chain.push(id);
        }
        // depth 0,1,2 doldu; bir sonraki depth 3 > 2 -> red.
        let candidate = Uuid::new_v4();
        let err = err_of(tree.add_node(candidate, chain.last().copied()));
        let Some(err) = err else {
            unreachable!("derinlik tavani zorlanmali")
        };
        assert!(matches!(err, HierarchyError::MaxDepthExceeded { depth: 3, max: 2 }));
        let denial = err.to_denial(candidate);
        assert_eq!(denial.code, "depth_cap");
        assert_eq!(denial.agent_id, candidate);
        assert_eq!(tree.len(), 3, "reddedilen spawn agaca yazilmamali");
    }

    #[test]
    fn fanout_cap_denies_spawn() {
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 5, max_fanout: 2 });
        let root = Uuid::new_v4();
        assert!(tree.add_node(root, None).is_ok());
        assert!(tree.add_node(Uuid::new_v4(), Some(root)).is_ok());
        assert!(tree.add_node(Uuid::new_v4(), Some(root)).is_ok());

        let candidate = Uuid::new_v4();
        let Some(err) = err_of(tree.add_node(candidate, Some(root))) else {
            unreachable!("fan-out tavani zorlanmali")
        };
        assert!(matches!(err, HierarchyError::MaxFanoutExceeded { fanout: 3, max: 2, .. }));
        assert_eq!(err.to_denial(candidate).code, "fanout_cap");
        assert_eq!(tree.fanout_of(&root), 2);
    }

    #[test]
    fn persona_override_only_narrows() {
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 5, max_fanout: 8 });
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(tree.add_node(root, None).is_ok());
        assert!(tree.add_node(child, Some(root)).is_ok());

        // Persona max_depth = 1 -> derinlik 2 reddedilir.
        assert!(matches!(
            tree.check_spawn(Some(child), Some(1)),
            Err(HierarchyError::MaxDepthExceeded { .. })
        ));
        // Override sistem tavanini genisletemez.
        assert_eq!(SpawnLimits::default().with_depth_override(Some(99)).max_depth, 5);
        // Override yoksa gecer.
        assert_eq!(tree.check_spawn(Some(child), None).ok(), Some(2));
    }

    #[test]
    fn check_spawn_is_side_effect_free() {
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 5, max_fanout: 8 });
        let root = Uuid::new_v4();
        assert!(tree.add_node(root, None).is_ok());
        assert_eq!(tree.check_spawn(Some(root), None).ok(), Some(1));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.fanout_of(&root), 0);
    }

    #[test]
    fn root_spawn_needs_no_parent() {
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 0, max_fanout: 0 });
        assert_eq!(tree.check_spawn(None, None).ok(), Some(0));
    }

    #[test]
    fn unknown_parent_is_denied() {
        let tree = HierarchyTree::new(DEFAULT_MAX_DEPTH);
        let ghost = Uuid::new_v4();
        let candidate = Uuid::new_v4();
        let Some(err) = err_of(tree.add_node(candidate, Some(ghost))) else {
            unreachable!("olmayan ebeveyn reddedilmeli")
        };
        assert!(matches!(err, HierarchyError::ParentNotFound(_)));
        assert_eq!(err.to_denial(candidate).code, "spawn_parent_missing");
    }

    #[test]
    fn restore_node_rebuilds_from_agents_depth() {
        // agents.depth kolonundan geri kurulum: tavan kontrolu yapilmaz.
        let tree = HierarchyTree::with_limits(SpawnLimits { max_depth: 1, max_fanout: 1 });
        let root = Uuid::new_v4();
        let mid = Uuid::new_v4();
        let leaf = Uuid::new_v4();
        assert!(tree.restore_node(root, None, 0).is_ok());
        assert!(tree.restore_node(mid, Some(root), 1).is_ok());
        assert!(tree.restore_node(leaf, Some(mid), 2).is_ok());

        assert_eq!(tree.get_depth(&leaf), Some(2));
        assert_eq!(tree.get_children(&root), vec![mid]);
        assert_eq!(tree.ancestors_of(&leaf), vec![mid, root]);
        // Tavan ihlali replay'de reddedilmez ama dogrulamada raporlanir.
        assert!(tree.validate_dag().is_err());
    }

    #[test]
    fn duplicate_node_is_rejected() {
        let tree = HierarchyTree::new(DEFAULT_MAX_DEPTH);
        let id = Uuid::new_v4();
        assert!(tree.add_node(id, None).is_ok());
        assert!(matches!(tree.add_node(id, None), Err(HierarchyError::DuplicateNode(_))));
        assert!(matches!(tree.restore_node(id, None, 0), Err(HierarchyError::DuplicateNode(_))));
    }

    #[test]
    fn cycle_is_rejected() {
        let tree = HierarchyTree::new(DEFAULT_MAX_DEPTH);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert!(tree.add_node(a, None).is_ok());
        assert!(tree.add_node(b, Some(a)).is_ok());
        // a'yi b'nin cocugu yapmak dongu olurdu.
        assert!(matches!(
            tree.check_spawn(Some(b), None).and_then(|_| tree.add_node(a, Some(b))),
            Err(HierarchyError::CycleDetected { .. }) | Err(HierarchyError::DuplicateNode(_))
        ));
    }

    #[test]
    fn valid_tree_passes_validation() {
        let tree = HierarchyTree::with_limits(SpawnLimits::default());
        let root = Uuid::new_v4();
        assert!(tree.add_node(root, None).is_ok());
        for _ in 0..DEFAULT_MAX_FANOUT {
            assert!(tree.add_node(Uuid::new_v4(), Some(root)).is_ok());
        }
        assert!(tree.validate_dag().is_ok());
        assert_eq!(tree.fanout_of(&root), DEFAULT_MAX_FANOUT);
    }
}
