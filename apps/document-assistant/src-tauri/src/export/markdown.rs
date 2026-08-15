use anyhow::Result;

use crate::analysis::CitedFact;

use super::ExportPayload;

pub fn render_enhanced_markdown(payload: &ExportPayload) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!("# {}\n\n", payload.summary.theme));
    output.push_str("## 分析元数据\n\n");
    output.push_str(&format!("- 原文件：{}\n", payload.metadata.file_name));
    output.push_str(&format!("- 格式：{}\n", payload.metadata.format));
    output.push_str(&format!("- 分析时间（Unix）：{}\n", payload.metadata.analysis_at));
    output.push_str(&format!(
        "- 视觉模型：{}\n",
        payload.metadata.vision_model.as_deref().unwrap_or("未启用")
    ));
    output.push_str(&format!("- 文本模型：{}\n", payload.metadata.text_model));
    if !payload.failed_pages.is_empty() {
        output.push_str(&format!(
            "- 失败页面：{}\n",
            payload
                .failed_pages
                .iter()
                .map(|page| format!("第 {page} 页"))
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    output.push_str("\n## 执行摘要\n\n");
    output.push_str(&payload.summary.executive_summary);
    output.push_str("\n\n");

    if !payload.summary.logical_outline.is_empty() {
        output.push_str("## 内容脉络\n\n");
        for item in &payload.summary.logical_outline {
            output.push_str(&format!(
                "- **{}**（第 {}–{} 页）：{}\n",
                item.heading, item.page_start, item.page_end, item.summary
            ));
        }
        output.push('\n');
    }
    render_facts(&mut output, "关键事实", &payload.summary.key_facts);
    render_facts(&mut output, "风险与疑点", &payload.summary.risks);
    render_facts(&mut output, "行动建议", &payload.summary.action_items);

    if !payload.page_analyses.is_empty() {
        output.push_str("## 逐页视觉分析\n\n");
        for page in &payload.page_analyses {
            output.push_str(&format!("### 第 {} 页\n\n{}\n\n", page.page_number, page.summary));
        }
    }
    if !payload.summary.analysis_limitations.is_empty() {
        output.push_str("## 分析范围与限制\n\n");
        for limitation in &payload.summary.analysis_limitations {
            output.push_str(&format!("- {limitation}\n"));
        }
        output.push('\n');
    }
    output.push_str("## AnyDoc 原始 Markdown\n\n");
    output.push_str(&payload.original_markdown);
    output.push('\n');
    Ok(output)
}

fn render_facts(output: &mut String, heading: &str, facts: &[CitedFact]) {
    if facts.is_empty() {
        return;
    }
    output.push_str(&format!("## {heading}\n\n"));
    for fact in facts {
        let sources = fact
            .evidence
            .iter()
            .map(|evidence| format!("第 {} 页", evidence.page_number))
            .collect::<Vec<_>>()
            .join("、");
        if sources.is_empty() {
            output.push_str(&format!("- {}\n", fact.text));
        } else {
            output.push_str(&format!("- {}（来源：{}）\n", fact.text, sources));
        }
    }
    output.push('\n');
}
