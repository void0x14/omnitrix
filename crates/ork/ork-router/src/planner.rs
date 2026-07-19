use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubTaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub status: SubTaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub task: String,
    pub sub_tasks: Vec<SubTask>,
}

impl Plan {
    pub fn new(task: &str, sub_tasks: Vec<SubTask>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            task: task.to_string(),
            sub_tasks,
        }
    }

    pub fn entry_points(&self) -> Vec<&SubTask> {
        self.sub_tasks
            .iter()
            .filter(|st| st.dependencies.is_empty())
            .collect()
    }

    pub fn ready_tasks(&self) -> Vec<&SubTask> {
        self.sub_tasks
            .iter()
            .filter(|st| {
                st.status == SubTaskStatus::Pending
                    && st
                        .dependencies
                        .iter()
                        .all(|dep_id| {
                            self.sub_tasks
                                .iter()
                                .any(|t| t.id == *dep_id && t.status == SubTaskStatus::Completed)
                        })
            })
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.sub_tasks.iter().all(|st| st.status == SubTaskStatus::Completed)
    }
}

pub struct Planner;

impl Planner {
    pub fn new() -> Self {
        Self
    }

    pub async fn generate_plan(&self, task: &str) -> Result<Plan> {
        info!("generating plan for task: {}", task);

        let sub_tasks = vec![
            SubTask {
                id: "analysis".into(),
                description: format!("analyze task requirements: {}", task),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "research".into(),
                description: "gather relevant context and data".into(),
                dependencies: vec!["analysis".into()],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "execute".into(),
                description: format!("execute primary work for: {}", task),
                dependencies: vec!["research".into()],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "verify".into(),
                description: "verify execution results".into(),
                dependencies: vec!["execute".into()],
                status: SubTaskStatus::Pending,
            },
        ];

        let plan = Plan::new(task, sub_tasks);
        info!("plan {} has {} sub-tasks", plan.id, plan.sub_tasks.len());
        Ok(plan)
    }
}
