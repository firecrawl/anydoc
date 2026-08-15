use crate::search::ContextPage;

use super::service::ChatTurn;

pub fn build_chat_prompt(question: &str, pages: &[ContextPage], recent: &[ChatTurn]) -> String {
    let context = pages
        .iter()
        .map(|page| {
            format!(
                "<page number=\"{}\">\n标题：{}\n文本：{}\n视觉摘要：{}\n</page>",
                page.page_number,
                page.heading.as_deref().unwrap_or("无"),
                page.text,
                page.visual_summary.as_deref().unwrap_or("无"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let history = recent
        .iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|turn| format!("{}：{}", turn.role, turn.content))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "你是严格依据文档作答的助手。只可使用 <page> 内容，不得使用外部知识或猜测。\n\
         每个事实必须引用提供的页码。证据不足时 answer 必须精确写成“文档中未找到相关信息”，grounded=false，citations=[]。\n\
         返回符合 JSON Schema 的对象。\n\n最近对话：\n{history}\n\n文档证据：\n{context}\n\n问题：{question}"
    )
}
