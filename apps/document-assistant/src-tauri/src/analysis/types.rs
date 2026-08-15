use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceRef {
    pub page_number: u32,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UncertainItem {
    pub description: String,
    pub reason: String,
    pub page_number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualElement {
    pub kind: String,
    pub description: String,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogicalRelation {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageAnalysis {
    pub page_number: u32,
    pub title: Option<String>,
    pub summary: String,
    pub visual_elements: Vec<VisualElement>,
    pub logical_relations: Vec<LogicalRelation>,
    pub key_facts: Vec<String>,
    pub uncertain_items: Vec<UncertainItem>,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitedFact {
    pub text: String,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutlineItem {
    pub heading: String,
    pub summary: String,
    pub page_start: u32,
    pub page_end: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentSummary {
    pub schema_version: u32,
    pub theme: String,
    pub executive_summary: String,
    pub logical_outline: Vec<OutlineItem>,
    pub key_facts: Vec<CitedFact>,
    pub risks: Vec<CitedFact>,
    pub action_items: Vec<CitedFact>,
    pub analysis_limitations: Vec<String>,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitedAnswer {
    pub answer: String,
    pub citations: Vec<EvidenceRef>,
}
