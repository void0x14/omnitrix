//! Paralel sorgu — sistem pas geçişi (paralel_query aşaması).
//!
//! AI bu aşamada karar VEREMEZ: yapı taşlarının hangilerinin paralel,
//! hangilerinin sıralı olduğunu bu deterministik analiz belirler.
//!
//! Girdi formatı (`building_blocks` dosyası — decompose aşamasında yazılır):
//! JSONL satırları:
//!   {"id":"b1","files":["src/a.rs"],"depends":[]}
//!   {"id":"b2","files":["src/b.rs"],"depends":["b1"]}
//!
//! Kural (deterministik):
//! - `depends` alanı açık bağımlılık bildirir → sıralı.
//! - `files` ortak dosya paylaşan bloklar → çakışma → sıralı.
//! - Hiçbir bağımlılığı/çakışması olmayan bloklar → paralel grup.
//! - Dosya okunamaz/bozuk ise: tüm bloklar sıralı (fail-safe).

use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildingBlock {
    pub id: String,
    /// Blokta yapılacak işin açıklaması (decompose aşamasında model yazar).
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub depends: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionGraph {
    /// Sıralı aşamalar; her aşama içinde paralel çalışabilen blok grupları.
    pub sequence: Vec<ParallelGroup>,
    pub total_blocks: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParallelGroup {
    pub parallel: Vec<String>,
}

/// `building_blocks` dosyasını okuyup deterministik yürütme grafiği üretir.
pub fn analyze(blocks_file: &Path) -> ExecutionGraph {
    let blocks: Vec<BuildingBlock> = std::fs::read_to_string(blocks_file)
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str::<BuildingBlock>(l).ok())
                .collect()
        })
        .unwrap_or_default();

    if blocks.is_empty() {
        // Dosya yok/boş/bozuk: fail-safe — sıralı (yürütme modelde kalır).
        return ExecutionGraph {
            sequence: vec![],
            total_blocks: 0,
        };
    }

    let by_id: HashMap<&str, &BuildingBlock> = blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let file_owner: HashMap<&str, Vec<&str>> = {
        let mut m: HashMap<&str, Vec<&str>> = HashMap::new();
        for b in &blocks {
            for f in &b.files {
                m.entry(f.as_str()).or_default().push(b.id.as_str());
            }
        }
        m
    };

    // Doğrudan bağımlılık + dosya çakışması → sıralı çift.
    let conflicts: HashSet<(&str, &str)> = {
        let mut set = HashSet::new();
        for b in &blocks {
            for dep in &b.depends {
                if by_id.contains_key(dep.as_str()) {
                    set.insert((dep.as_str(), b.id.as_str()));
                }
            }
            for f in &b.files {
                if let Some(owners) = file_owner.get(f.as_str()) {
                    for other in owners {
                        if *other != b.id.as_str() {
                            set.insert((other, b.id.as_str()));
                        }
                    }
                }
            }
        }
        set
    };

    // Kalan bloklar arasında paralellik: a ve b paraleldir ⇔ (a,b) ve (b,a)
    // çakışma kümesinde değil. Basit deterministik grup: sırayla tara,
    // mevcut grubun TÜM üyeleriyle çakışmayan blok gruba eklenir.
    let mut remaining: Vec<&str> = blocks.iter().map(|b| b.id.as_str()).collect();
    let mut sequence: Vec<ParallelGroup> = Vec::new();
    while !remaining.is_empty() {
        let mut group: Vec<&str> = vec![remaining[0]];
        let pivot = remaining[0];
        let mut rest: Vec<&str> = Vec::new();
        for candidate in remaining.iter().skip(1) {
            let conflicts_with_group = group.iter().any(|g| {
                conflicts.contains(&(g, candidate)) || conflicts.contains(&(candidate, g))
            });
            if conflicts_with_group || *candidate == pivot {
                rest.push(candidate);
            } else {
                group.push(candidate);
            }
        }
        group.sort_unstable();
        sequence.push(ParallelGroup {
            parallel: group.iter().map(|s| s.to_string()).collect(),
        });
        remaining = rest;
    }

    ExecutionGraph {
        sequence,
        total_blocks: blocks.len(),
    }
}

/// Grafiğin insan+AI okur özeti (kanıt detayına yazılır).
pub fn graph_summary(graph: &ExecutionGraph) -> String {
    if graph.total_blocks == 0 {
        return "yapı taşı dosyası okunamadı; sıralı yürütme (fail-safe)".to_string();
    }
    let groups: Vec<String> = graph
        .sequence
        .iter()
        .map(|g| {
            if g.parallel.len() > 1 {
                format!("paralel{{{}}}", g.parallel.join(","))
            } else {
                g.parallel[0].clone()
            }
        })
        .collect();
    format!("{} blok → {}", graph.total_blocks, groups.join(" → "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_blocks(lines: &[&str]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("flow_par_test_{}", std::process::id()));
        std::fs::write(&p, lines.join("\n")).ok();
        p
    }

    #[test]
    fn independent_blocks_are_parallel() {
        let f = write_blocks(&[
            r#"{"id":"b1","files":["src/a.rs"],"depends":[]}"#,
            r#"{"id":"b2","files":["src/b.rs"],"depends":[]}"#,
        ]);
        let g = analyze(&f);
        assert_eq!(g.total_blocks, 2);
        assert_eq!(g.sequence.len(), 1);
        assert_eq!(g.sequence[0].parallel.len(), 2);
    }

    #[test]
    fn dependency_forces_sequence() {
        let f = write_blocks(&[
            r#"{"id":"b1","files":["src/a.rs"],"depends":[]}"#,
            r#"{"id":"b2","files":["src/b.rs"],"depends":["b1"]}"#,
        ]);
        let g = analyze(&f);
        assert_eq!(g.sequence.len(), 2);
    }

    #[test]
    fn shared_file_forces_sequence() {
        let f = write_blocks(&[
            r#"{"id":"b1","files":["src/a.rs"],"depends":[]}"#,
            r#"{"id":"b2","files":["src/a.rs"],"depends":[]}"#,
        ]);
        let g = analyze(&f);
        assert_eq!(g.sequence.len(), 2);
    }

    #[test]
    fn missing_file_is_fail_safe() {
        let g = analyze(Path::new("/nonexistent/blocks.jsonl"));
        assert_eq!(g.total_blocks, 0);
        assert!(g.sequence.is_empty());
    }
}
