pub mod pipeline;
pub mod prompts;
pub mod schema;
pub mod synthesis;
pub mod types;
pub mod vision;

pub use pipeline::{AnalysisPipelineInput, run_analysis_pipeline};
pub use schema::{
    document_summary_schema, page_analysis_schema, parse_page_analysis, validate_evidence_pages,
    validate_page_number,
};
pub use types::*;
pub use vision::VisionAnalyzer;

#[cfg(test)]
mod schema_test;

#[cfg(test)]
mod vision_test;

#[cfg(test)]
mod synthesis_test;

#[cfg(test)]
mod pipeline_test;
