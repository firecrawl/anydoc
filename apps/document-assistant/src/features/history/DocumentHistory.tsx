import { useState } from 'react';
import type { HistoryDocument as HistoryDocumentData } from './historyApi';

export type HistoryDocument = HistoryDocumentData;

interface DocumentHistoryProps {
  documents: HistoryDocument[];
  onDelete: (documentId: string) => Promise<void>;
  onClearAll: () => Promise<void>;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function DocumentHistory({
  documents,
  onDelete,
  onClearAll,
}: DocumentHistoryProps) {
  const [pendingDelete, setPendingDelete] = useState<string>();
  const [confirmClear, setConfirmClear] = useState(false);
  const totalBytes = documents.reduce((sum, document) => sum + document.cacheSize, 0);

  return (
    <section className="document-history" aria-labelledby="history-title">
      <div className="history-heading">
        <div>
          <p className="eyebrow">Local library</p>
          <h2 id="history-title">文档历史</h2>
        </div>
        <button type="button" onClick={() => setConfirmClear(true)}>
          清理全部缓存
        </button>
      </div>

      {confirmClear ? (
        <div className="history-confirm" role="alertdialog" aria-label="确认清理全部缓存">
          <strong>
            将清理 {documents.length} 个文档，共 {formatBytes(totalBytes)}
          </strong>
          <span>只删除分析缓存，不删除原文件</span>
          <div>
            <button type="button" onClick={() => setConfirmClear(false)}>
              取消
            </button>
            <button type="button" onClick={() => void onClearAll()}>
              确认清理全部缓存
            </button>
          </div>
        </div>
      ) : null}

      <div className="history-list">
        {documents.map((document) => (
          <article key={document.id} className="history-item">
            <div>
              <strong>{document.fileName}</strong>
              <span>
                {document.format.toUpperCase()} · {document.status} ·{' '}
                {formatBytes(document.cacheSize)}
              </span>
            </div>
            {pendingDelete === document.id ? (
              <div className="inline-confirm">
                <span>只删除分析缓存，不删除原文件</span>
                <button type="button" onClick={() => setPendingDelete(undefined)}>
                  取消
                </button>
                <button type="button" onClick={() => void onDelete(document.id)}>
                  确认删除缓存
                </button>
              </div>
            ) : (
              <button type="button" onClick={() => setPendingDelete(document.id)}>
                删除缓存
              </button>
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
