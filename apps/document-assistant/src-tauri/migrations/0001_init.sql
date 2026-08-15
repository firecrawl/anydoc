CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY NOT NULL,
    source_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    format TEXT NOT NULL,
    sha256 TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    markdown_path TEXT,
    cache_size INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS pages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL,
    image_path TEXT,
    width INTEGER,
    height INTEGER,
    markdown TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    UNIQUE(document_id, page_number)
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    message TEXT NOT NULL DEFAULT '',
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS model_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL,
    base_url TEXT NOT NULL,
    model TEXT NOT NULL,
    supports_vision INTEGER NOT NULL DEFAULT 0,
    timeout_secs INTEGER NOT NULL DEFAULT 120,
    max_concurrency INTEGER NOT NULL DEFAULT 2,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    citations_json TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS page_search USING fts5(
    document_id UNINDEXED,
    page_number UNINDEXED,
    content,
    tokenize = 'unicode61'
);

CREATE INDEX IF NOT EXISTS idx_documents_updated_at ON documents(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_pages_document_id ON pages(document_id, page_number);
CREATE INDEX IF NOT EXISTS idx_tasks_document_id ON tasks(document_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_conversations_document_id ON conversations(document_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id, created_at);
