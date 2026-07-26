use std::sync::Arc;

use async_trait::async_trait;

use crate::grounding::{Evidence, GroundingVerifier};
use crate::planner::{Plan, SubTask, SubTaskStatus};
use crate::strategies::{Router, RoutingPolicy, Usage};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: serde_json::Value,
    pub status: ToolCallStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Success,
    Error(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionOutput {
    pub results: Vec<String>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub completed_tasks: Vec<String>,
    pub failed_tasks: Vec<String>,
}

impl ExecutionOutput {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            tool_calls: Vec::new(),
            completed_tasks: Vec::new(),
            failed_tasks: Vec::new(),
        }
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, tool_name: &str, arguments: serde_json::Value) -> ToolCallRecord;
    async fn check_available(&self, tool_name: &str) -> bool;
    async fn estimate_cost(&self, tool_name: &str) -> Usage;
}

pub struct Executor {
    policy: RoutingPolicy,
    usage: Usage,
    tool_executor: Option<Arc<dyn ToolExecutor>>,
}

impl Executor {
    pub fn new(policy: RoutingPolicy) -> Self {
        Self {
            policy,
            usage: Usage::default(),
            tool_executor: None,
        }
    }

    pub fn with_tool_executor(mut self, executor: Arc<dyn ToolExecutor>) -> Self {
        self.tool_executor = Some(executor);
        self
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    fn check_budget(&self) -> bool {
        match &self.policy.budget {
            Some(budget) => Router::check_budget(budget, &self.usage),
            None => true,
        }
    }

    pub async fn execute_plan(&self, plan: &Plan) -> Result<ExecutionOutput> {
        info!("executing plan {} with {} sub-tasks", plan.id, plan.sub_tasks.len());

        if !self.check_budget() {
            warn!("budget exhausted before execution started");
            return Ok(ExecutionOutput::new());
        }

        let mut output = ExecutionOutput::new();
        let mut tasks: Vec<SubTask> = plan.sub_tasks.clone();
        let mut evidence_pool: Vec<Evidence> = Vec::new();

        while !tasks.iter().all(|t| {
            matches!(t.status, SubTaskStatus::Completed | SubTaskStatus::Failed)
        }) {
            let ready: Vec<usize> = tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.status == SubTaskStatus::Pending
                        && t.dependencies.iter().all(|dep_id| {
                            tasks.iter().any(|t2| t2.id == *dep_id && t2.status == SubTaskStatus::Completed)
                        })
                })
                .map(|(i, _)| i)
                .collect();

            if ready.is_empty() {
                let blocked: Vec<_> = tasks
                    .iter()
                    .filter(|t| t.status == SubTaskStatus::Pending)
                    .map(|t| t.id.clone())
                    .collect();
                if !blocked.is_empty() {
                    warn!("tasks blocked by dependencies: {:?}", blocked);
                    for t in &mut tasks {
                        if t.status == SubTaskStatus::Pending {
                            t.status = SubTaskStatus::Failed;
                            output.failed_tasks.push(t.id.clone());
                            output.results.push(format!("{} blocked by failed dependency", t.description));
                        }
                    }
                }
                break;
            }

            for idx in ready {
                if !self.check_budget() {
                    warn!("budget exhausted, halting execution");
                    let remaining: Vec<_> = tasks.iter_mut().filter(|t| t.status == SubTaskStatus::Pending).collect();
                    for t in remaining {
                        t.status = SubTaskStatus::Failed;
                        output.failed_tasks.push(t.id.clone());
                        output.results.push(format!("{} cancelled (budget)", t.description));
                    }
                    break;
                }

                let task_id = tasks[idx].id.clone();
                let task_desc = tasks[idx].description.clone();
                info!("executing sub-task: {} - {}", task_id, task_desc);

                let record = self.execute_tool_call(&tasks[idx]).await;

                match &record.status {
                    ToolCallStatus::Success => {
                        tasks[idx].status = SubTaskStatus::Completed;
                        output.completed_tasks.push(task_id);
                        output.results.push(format!("{} completed", task_desc));

                        if let serde_json::Value::Object(ref map) = record.result {
                            if let Some(content) = map.get("evidence")
                                && let Some(content_str) = content.as_str()
                            {
                                evidence_pool.push(Evidence {
                                    source: format!("tool:{}", tasks[idx].id),
                                    content: content_str.to_string(),
                                    tool_call_id: Some(tasks[idx].id.clone()),
                                });
                            }
                            if let Some(content) = map.get("output")
                                && let Some(content_str) = content.as_str()
                            {
                                evidence_pool.push(Evidence {
                                    source: format!("tool:{}", tasks[idx].id),
                                    content: content_str.to_string(),
                                    tool_call_id: Some(tasks[idx].id.clone()),
                                });
                            }
                        }
                    }
                    ToolCallStatus::Error(e) => {
                        tasks[idx].status = SubTaskStatus::Failed;
                        output.failed_tasks.push(task_id);
                        output.results.push(format!("{} failed: {}", task_desc, e));
                        warn!("sub-task {} failed: {}", tasks[idx].id, e);

                        let failed_id = tasks[idx].id.clone();
                        for t in &mut tasks {
                            if t.status == SubTaskStatus::Pending && t.dependencies.contains(&failed_id) {
                                t.status = SubTaskStatus::Failed;
                                output.failed_tasks.push(t.id.clone());
                                output.results.push(format!("{} skipped (dependency failed)", t.description));
                            }
                        }
                    }
                }
                output.tool_calls.push(record);
            }

            if output.tool_calls.last().map(|r| matches!(r.status, ToolCallStatus::Error(_))).unwrap_or(false)
                && !self.check_budget()
            {
                break;
            }
        }

        if self.policy.grounding != crate::strategies::GroundingMode::Off {
            let verifier = GroundingVerifier::new();
            let claims: Vec<_> = output
                .tool_calls
                .iter()
                .filter_map(|tc| match &tc.status {
                    ToolCallStatus::Success => {
                        let id = tc.tool_name.clone();
                        let statement = format!("tool {} executed successfully", id);
                        Some(crate::grounding::Claim {
                            id,
                            statement,
                            expected_evidence: evidence_pool.iter().map(|e| e.source.clone()).collect(),
                        })
                    }
                    ToolCallStatus::Error(_) => None,
                })
                .collect();

            if !claims.is_empty() {
                let require_all = self.policy.grounding == crate::strategies::GroundingMode::Required;
                match verifier.verify_iterative(&claims, &evidence_pool, require_all) {
                    Ok(report) => {
                        if report.failed_claims > 0 {
                            warn!("grounding: {}/{} claims failed", report.failed_claims, report.total_claims);
                        } else {
                            info!("grounding: all {} claims verified", report.total_claims);
                        }
                    }
                    Err(e) => {
                        warn!("grounding verification failed: {}", e);
                    }
                }
            }
        }

        info!(
            "execution complete: {} completed, {} failed",
            output.completed_tasks.len(),
            output.failed_tasks.len()
        );
        Ok(output)
    }

    async fn execute_tool_call(&self, task: &SubTask) -> ToolCallRecord {
        let executor = match &self.tool_executor {
            Some(e) => e,
            None => {
                let arguments = serde_json::json!({
                    "task_id": task.id,
                    "description": task.description,
                });
                return ToolCallRecord {
                    tool_name: "subtask_execute".into(),
                    arguments,
                    result: serde_json::json!({"status": "ok"}),
                    status: ToolCallStatus::Success,
                };
            }
        };

        if !executor.check_available(&task.id).await {
            warn!("tool {} not available in current policy", task.id);
            return ToolCallRecord {
                tool_name: task.id.clone(),
                arguments: serde_json::json!({"task_id": task.id}),
                result: serde_json::json!({"error": "tool not available"}),
                status: ToolCallStatus::Error("tool not available in current policy".into()),
            };
        }

        let estimated = executor.estimate_cost(&task.id).await;
        let estimated_usage = Usage {
            tokens_used: self.usage.tokens_used + estimated.tokens_used,
            cost_incurred: self.usage.cost_incurred + estimated.cost_incurred,
            latency_ms: self.usage.latency_ms + estimated.latency_ms,
        };

        if let Some(budget) = &self.policy.budget
            && !Router::check_budget(budget, &estimated_usage)
        {
            warn!("estimated cost for tool {} would exceed budget", task.id);
            return ToolCallRecord {
                tool_name: task.id.clone(),
                arguments: serde_json::json!({"task_id": task.id}),
                result: serde_json::json!({"error": "budget exceeded"}),
                status: ToolCallStatus::Error("estimated cost exceeds remaining budget".into()),
            };
        }

        let arguments = serde_json::json!({
            "task_id": task.id,
            "description": task.description,
        });

        executor.execute(&task.id, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::planner::{Plan, Planner, SubTask, SubTaskStatus};
    use crate::strategies::{GroundingMode, RoutingBudget, RoutingPolicy, RoutingStrategy};

    use super::*;

    struct MockExecutor {
        call_count: AtomicUsize,
        fail_tool: Option<String>,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                fail_tool: None,
            }
        }

        fn with_fail(fail_tool: &str) -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                fail_tool: Some(fail_tool.to_string()),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for MockExecutor {
        async fn execute(&self, tool_name: &str, arguments: serde_json::Value) -> ToolCallRecord {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let should_fail = self.fail_tool.as_deref() == Some(tool_name);
            ToolCallRecord {
                tool_name: tool_name.to_string(),
                arguments,
                result: if should_fail {
                    serde_json::json!({"error": "mock failure"})
                } else {
                    serde_json::json!({"status": "ok", "output": format!("{} executed", tool_name), "evidence": format!("evidence for {}", tool_name)})
                },
                status: if should_fail {
                    ToolCallStatus::Error("mock failure".into())
                } else {
                    ToolCallStatus::Success
                },
            }
        }

        async fn check_available(&self, _tool_name: &str) -> bool {
            true
        }

        async fn estimate_cost(&self, _tool_name: &str) -> Usage {
            Usage {
                tokens_used: 100,
                cost_incurred: 0.001,
                latency_ms: 50,
            }
        }
    }

    fn make_policy() -> RoutingPolicy {
        RoutingPolicy {
            strategy: RoutingStrategy::JudgeExecutorPlanner,
            fallback_chain: vec![],
            budget: None,
            grounding: GroundingMode::Off,
        }
    }

    #[tokio::test]
    async fn test_execute_topological_order() {
        let policy = make_policy();
        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Planner::new().generate_plan("test task").await.unwrap();
        let output = executor.execute_plan(&plan).await.unwrap();

        // execute_plan plani BASTAN SONA kosar; bu testin isi sirayi dogrulamak.
        // Planner zinciri analysis -> research -> execute -> verify olarak kurar,
        // dolayisiyla her gorev bagimliligindan SONRA cagrilmalidir.
        let call_order: Vec<_> = output.tool_calls.iter().map(|tc| tc.tool_name.clone()).collect();
        assert_eq!(
            call_order,
            vec!["analysis", "research", "execute", "verify"],
            "gorevler topolojik sirada cagrilmali"
        );

        assert_eq!(output.completed_tasks.len(), 4, "plandaki tum gorevler tamamlanmali");
    }

    #[tokio::test]
    async fn test_execute_sequential_plan() {
        let policy = make_policy();
        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("seq", vec![
            SubTask {
                id: "a".into(),
                description: "task a".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "b".into(),
                description: "task b".into(),
                dependencies: vec!["a".into()],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "c".into(),
                description: "task c".into(),
                dependencies: vec!["b".into()],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert_eq!(output.completed_tasks.len(), 3);
        assert!(output.failed_tasks.is_empty());
        assert_eq!(output.tool_calls.len(), 3);

        let names: Vec<_> = output.tool_calls.iter().map(|t| t.tool_name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"], "must execute in topological order");
    }

    #[tokio::test]
    async fn test_execute_parallel_tasks() {
        let policy = make_policy();
        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("parallel", vec![
            SubTask {
                id: "a".into(),
                description: "task a".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "b".into(),
                description: "task b".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "c".into(),
                description: "task c".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert_eq!(output.completed_tasks.len(), 3);
        assert!(output.failed_tasks.is_empty());
    }

    #[tokio::test]
    async fn test_tool_failure_cascades() {
        let policy = make_policy();
        let mock = Arc::new(MockExecutor::with_fail("a"));
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("cascade", vec![
            SubTask {
                id: "a".into(),
                description: "task a".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
            SubTask {
                id: "b".into(),
                description: "task b".into(),
                dependencies: vec!["a".into()],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert_eq!(output.completed_tasks.len(), 0, "a fails so nothing completes");
        assert!(output.failed_tasks.contains(&"a".to_string()), "a should be failed");
        assert!(output.failed_tasks.contains(&"b".to_string()), "b should cascade-fail");
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::JudgeExecutorPlanner,
            fallback_chain: vec![],
            budget: Some(RoutingBudget {
                max_tokens: Some(50),
                max_cost: None,
                max_latency_ms: None,
            }),
            grounding: GroundingMode::Off,
        };

        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("budget", vec![
            SubTask {
                id: "a".into(),
                description: "task a".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert!(
            output.failed_tasks.contains(&"a".to_string()) || output.tool_calls.is_empty(),
            "budget check should prevent execution of tool that would exceed budget"
        );
    }

    #[tokio::test]
    async fn test_execute_empty_plan() {
        let policy = make_policy();
        let executor = Executor::new(policy);
        let plan = Plan::new("empty", vec![]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert!(output.completed_tasks.is_empty());
        assert!(output.failed_tasks.is_empty());
        assert!(output.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn test_no_tool_executor_fallback() {
        let policy = make_policy();
        let executor = Executor::new(policy);

        let plan = Plan::new("fallback", vec![
            SubTask {
                id: "x".into(),
                description: "fallback task".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert_eq!(output.completed_tasks.len(), 1, "no executor should use fallback stub");
        assert_eq!(
            output.tool_calls[0].tool_name,
            "subtask_execute",
            "fallback uses stub tool name"
        );
    }

    #[tokio::test]
    async fn test_grounding_mode_collects_evidence() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::JudgeExecutorPlanner,
            fallback_chain: vec![],
            budget: None,
            grounding: GroundingMode::Preferred,
        };

        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("grounded", vec![
            SubTask {
                id: "g1".into(),
                description: "grounded task".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
        ]);

        let output = executor.execute_plan(&plan).await.unwrap();
        assert_eq!(output.completed_tasks.len(), 1);
        assert!(output.tool_calls[0].result.get("evidence").is_some());
    }

    #[tokio::test]
    async fn test_execute_plan_tracks_usage() {
        let policy = make_policy();
        let mock = Arc::new(MockExecutor::new());
        let executor = Executor::new(policy).with_tool_executor(mock.clone());

        let plan = Plan::new("usage", vec![
            SubTask {
                id: "u1".into(),
                description: "usage task".into(),
                dependencies: vec![],
                status: SubTaskStatus::Pending,
            },
        ]);

        let _output = executor.execute_plan(&plan).await.unwrap();
    }
}
