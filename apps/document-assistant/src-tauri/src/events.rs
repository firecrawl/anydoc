use serde::Serialize;

pub const TASK_PROGRESS_EVENT: &str = "task-progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Queued,
    Parsing,
    Rendering,
    VisionAnalysis,
    TextSynthesis,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub document_id: String,
    pub stage: ProgressStage,
    pub completed: u32,
    pub total: u32,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::{ProgressStage, TaskProgressEvent};

    #[test]
    fn progress_stage_serializes_for_the_frontend_contract() {
        let event = TaskProgressEvent {
            document_id: "doc-1".into(),
            stage: ProgressStage::VisionAnalysis,
            completed: 2,
            total: 4,
            message: "正在理解第 2 页".into(),
        };

        let json = serde_json::to_value(event).expect("progress event serializes");
        assert_eq!(json["documentId"], "doc-1");
        assert_eq!(json["stage"], "vision_analysis");
    }
}
