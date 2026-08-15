use anyhow::Result;

use super::ExportPayload;

pub fn render_json(payload: &ExportPayload) -> Result<String> {
    Ok(serde_json::to_string_pretty(payload)?)
}
