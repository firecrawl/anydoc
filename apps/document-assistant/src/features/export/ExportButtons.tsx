import { useState } from 'react';
import { exportDocument as defaultExportDocument, type ExportFormat, type ExportReceipt } from './exportApi';

interface ExportButtonsProps {
  documentId: string;
  exportDocument?: (documentId: string, format: ExportFormat) => Promise<ExportReceipt | null>;
}

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

export function ExportButtons({ documentId, exportDocument = defaultExportDocument }: ExportButtonsProps) {
  const [pending, setPending] = useState<ExportFormat | null>(null);
  const [receipt, setReceipt] = useState<ExportReceipt | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async (format: ExportFormat) => {
    setPending(format);
    setError(null);
    try {
      const result = await exportDocument(documentId, format);
      if (result) setReceipt(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(null);
    }
  };

  return (
    <div className="export-area">
      <div className="export-buttons" aria-label="导出文档">
        <button type="button" disabled={pending !== null} onClick={() => void run('original_markdown')}>导出原始 Markdown</button>
        <button type="button" disabled={pending !== null} onClick={() => void run('enhanced_markdown')}>导出增强 Markdown</button>
        <button type="button" disabled={pending !== null} onClick={() => void run('structured_json')}>导出结构 JSON</button>
      </div>
      {pending ? <span role="status">正在准备导出…</span> : null}
      {receipt && !pending ? <span role="status">已保存到 {receipt.destination}（{formatSize(receipt.bytesWritten)}）</span> : null}
      {error ? <span role="alert">{error}</span> : null}
    </div>
  );
}
