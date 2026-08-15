use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::indexer::{SearchIndex, query_tokens};

#[derive(Debug, Clone, PartialEq)]
pub struct ContextPage {
    pub document_id: String,
    pub page_number: u32,
    pub heading: Option<String>,
    pub text: String,
    pub visual_summary: Option<String>,
    pub score: f64,
}

impl SearchIndex {
    pub fn retrieve_context(
        &self,
        document_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ContextPage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tokens = query_tokens(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = tokens
            .iter()
            .map(|token| format!("\"{}\"", token.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let connection =
            self.connection.lock().map_err(|_| anyhow::anyhow!("search index lock poisoned"))?;
        let mut statement = connection.prepare(
            "SELECT s.document_id, s.page_number, s.heading, s.text, s.visual_summary,
                    -bm25(page_search) AS score
             FROM page_search
             JOIN search_pages s ON s.document_id = page_search.document_id
                                AND s.page_number = page_search.page_number
             WHERE page_search MATCH ?1 AND page_search.document_id = ?2
             ORDER BY bm25(page_search), s.page_number
             LIMIT ?3",
        )?;
        let primary = statement
            .query_map(params![fts_query, document_id, limit as u32], context_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        if primary.is_empty() {
            return Ok(primary);
        }

        let mut results = BTreeMap::new();
        let primary_numbers = primary.iter().map(|page| page.page_number).collect::<HashSet<_>>();
        for page in primary {
            results.insert(page.page_number, page);
        }
        for page_number in primary_numbers {
            for neighbor in [page_number.checked_sub(1), page_number.checked_add(1)] {
                let Some(neighbor) = neighbor.filter(|number| *number > 0) else { continue };
                if results.len() >= limit || results.contains_key(&neighbor) {
                    continue;
                }
                let page = connection
                    .query_row(
                        "SELECT document_id, page_number, heading, text, visual_summary, 0.0
                     FROM search_pages WHERE document_id = ?1 AND page_number = ?2",
                        params![document_id, neighbor],
                        context_from_row,
                    )
                    .optional()?;
                if let Some(page) = page {
                    results.insert(neighbor, page);
                }
            }
        }
        let mut results = results.into_values().collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right.score.total_cmp(&left.score).then(left.page_number.cmp(&right.page_number))
        });
        Ok(results)
    }
}

fn context_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextPage> {
    Ok(ContextPage {
        document_id: row.get(0)?,
        page_number: row.get(1)?,
        heading: row.get(2)?,
        text: row.get(3)?,
        visual_summary: row.get(4)?,
        score: row.get(5)?,
    })
}
