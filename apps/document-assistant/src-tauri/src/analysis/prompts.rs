use super::vision::PageAnalysisInput;

pub fn build_page_prompt(input: &PageAnalysisInput) -> String {
    format!(
        "你是严谨的文档视觉分析器。分析第 {page} 页/幻灯片，结合页面图像与本地提取文字，\
只输出符合给定 JSON Schema 的对象。请识别图表、图片、流程、空间布局和文字之间的逻辑关系；\
不要凭空补全。任何无法确定的内容必须写入 uncertain_items（JSON 字段 uncertainItems），并说明原因。\
所有事实只能来自本页证据。\n\
上一标题：{previous}\n\
本页文字：\n{text}\n\
下一标题：{next}",
        page = input.page_number,
        previous = input.previous_heading.as_deref().unwrap_or("无"),
        text = input.page_text,
        next = input.next_heading.as_deref().unwrap_or("无"),
    )
}

pub fn build_repair_prompt(page_number: u32, raw: &str, validation_error: &str) -> String {
    format!(
        "下面是第 {page_number} 页分析的无效输出。不要重新分析图片，不要添加新事实。\
仅根据原输出修复为符合 JSON Schema 的 JSON 对象，并确保 pageNumber={page_number}。\n\
校验错误：{validation_error}\n原输出：\n{raw}"
    )
}
