use crate::executor::{ExecutionOutput, Executor};
use crate::judge::{JudgeVerdict, Judge};
use crate::planner::{Plan, Planner};
use crate::strategies::RoutingPolicy;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JepPhase {
    Plan,
    Execute,
    Judge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JepResult {
    pub plan: Plan,
    pub execution: ExecutionOutput,
    pub verdict: JudgeVerdict,
    pub iterations: u32,
}

pub struct JepEngine {
    planner: Planner,
    executor: Executor,
    judge: Judge,
    max_iterations: u32,
    policy: RoutingPolicy,
}

impl JepEngine {
    pub fn new(policy: RoutingPolicy) -> Self {
        Self {
            planner: Planner::new(),
            executor: Executor::new(policy.clone()),
            judge: Judge::new(None),
            max_iterations: 3,
            policy,
        }
    }

    pub fn with_max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = n;
        self
    }

    pub async fn run_cycle(&self, task: &str) -> Result<JepResult> {
        let mut last_result: Option<JepResult> = None;

        for iteration in 1..=self.max_iterations {
            info!("JEP iteration {}/{}", iteration, self.max_iterations);

            let plan = self.plan_phase(task).await?;
            let execution = self.execute_phase(&plan).await?;
            let verdict = self.judge_phase(&execution).await?;

            let result = JepResult {
                plan,
                execution,
                verdict: verdict.clone(),
                iterations: iteration,
            };

            if verdict.passed {
                info!("JEP cycle passed on iteration {}", iteration);
                return Ok(result);
            }

            last_result = Some(result);
            info!("JEP cycle iteration {} did not pass, re-trying", iteration);
        }

        last_result.ok_or_else(|| anyhow::anyhow!("JEP cycle produced no result"))
    }

    pub async fn plan_phase(&self, task: &str) -> Result<Plan> {
        info!("JEP plan phase for task: {}", task);
        self.planner.generate_plan(task).await
    }

    pub async fn execute_phase(&self, plan: &Plan) -> Result<ExecutionOutput> {
        info!("JEP execute phase with {} sub-tasks", plan.sub_tasks.len());
        self.executor.execute_plan(plan).await
    }

    pub async fn judge_phase(&self, output: &ExecutionOutput) -> Result<JudgeVerdict> {
        info!("JEP judge phase evaluating execution output");
        self.judge.evaluate(output).await
    }
}
