use crate::tasks::TaskRecord;

pub const TASK_PROGRESS_EVENT: &str = "task-progress";

pub fn task_progress_event(task: &TaskRecord) -> TaskRecord {
    task.clone()
}

#[cfg(test)]
mod tests {
    use crate::tasks::{TaskRecord, TaskStage};

    use super::task_progress_event;

    #[test]
    fn progress_stage_serializes_for_the_frontend_contract() {
        let mut task = TaskRecord::new("task-1", "doc-1", TaskStage::VisionAnalysis, 4);
        task.completed = 2;
        task.message = "正在理解第 2 页".into();

        let json =
            serde_json::to_value(task_progress_event(&task)).expect("progress event serializes");
        assert_eq!(json["id"], "task-1");
        assert_eq!(json["documentId"], "doc-1");
        assert_eq!(json["stage"], "vision_analysis");
    }
}
