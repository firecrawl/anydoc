use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::types::{DocumentSummary, EvidenceRef, PageAnalysis};

pub fn page_analysis_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(PageAnalysis))
        .expect("PageAnalysis schema always serializes")
}

pub fn document_summary_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(DocumentSummary))
        .expect("DocumentSummary schema always serializes")
}

pub fn parse_page_analysis(json: &str) -> Result<PageAnalysis> {
    let value: Value = serde_json::from_str(json).context("page analysis is not valid JSON")?;
    let schema = page_analysis_schema();
    let validator = jsonschema::validator_for(&schema).context("compile page analysis schema")?;
    if let Err(error) = validator.validate(&value) {
        bail!("page analysis schema violation: {error}");
    }
    let analysis: PageAnalysis = serde_json::from_value(value).context("decode page analysis")?;
    if analysis.summary.trim().is_empty() {
        bail!("page analysis summary cannot be empty");
    }
    if !analysis.confidence.is_finite() || !(0.0..=1.0).contains(&analysis.confidence) {
        bail!("page analysis confidence must be between 0 and 1");
    }
    Ok(analysis)
}

pub fn validate_page_number(analysis: &PageAnalysis, expected: u32) -> Result<()> {
    if analysis.page_number != expected {
        bail!("page number mismatch: expected {expected}, got {}", analysis.page_number);
    }
    Ok(())
}

pub fn validate_evidence_pages(evidence: &[EvidenceRef], total_pages: u32) -> Result<()> {
    for reference in evidence {
        if reference.page_number == 0 || reference.page_number > total_pages {
            bail!("evidence page {} is outside 1..={total_pages}", reference.page_number);
        }
    }
    Ok(())
}
