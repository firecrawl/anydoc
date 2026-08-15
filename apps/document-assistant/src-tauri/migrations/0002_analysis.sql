CREATE TABLE IF NOT EXISTS page_analyses (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL,
    analysis_json TEXT,
    raw_response TEXT,
    status TEXT NOT NULL,
    error TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(document_id, page_number)
);

CREATE TABLE IF NOT EXISTS document_analysis (
    document_id TEXT PRIMARY KEY NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    visual_content_analyzed INTEGER NOT NULL DEFAULT 0,
    summary_json TEXT,
    consent_at INTEGER,
    vision_profile_id TEXT,
    text_profile_id TEXT,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_page_analyses_document
    ON page_analyses(document_id, page_number);
