use serde_json::json;

use super::{parse_page_analysis, validate_page_number};

fn analysis_json(confidence: f32) -> String {
    json!({
        "pageNumber": 3,
        "title": "季度概览",
        "summary": "本页比较了收入与成本。",
        "visualElements": [],
        "logicalRelations": [],
        "keyFacts": ["收入增长"],
        "uncertainItems": [],
        "confidence": confidence
    })
    .to_string()
}

#[test]
fn rejects_confidence_outside_zero_to_one() {
    assert!(parse_page_analysis(&analysis_json(1.5)).is_err());
}

#[test]
fn rejects_a_page_number_mismatch() {
    let analysis = parse_page_analysis(&analysis_json(0.8)).expect("analysis parses");
    assert!(validate_page_number(&analysis, 4).is_err());
}

#[test]
fn rejects_an_empty_summary_and_unknown_fields() {
    let mut value: serde_json::Value =
        serde_json::from_str(&analysis_json(0.8)).expect("fixture parses");
    value["summary"] = json!("   ");
    assert!(parse_page_analysis(&value.to_string()).is_err());

    value["summary"] = json!("有效摘要");
    value["inventedField"] = json!(true);
    assert!(parse_page_analysis(&value.to_string()).is_err());
}
