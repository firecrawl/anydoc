import { invokeCommand } from '../../lib/desktop/api';

export type ExportFormat = 'original_markdown' | 'enhanced_markdown' | 'structured_json';

export interface ExportReceipt {
  destination: string;
  bytesWritten: number;
}

export function exportDocument(documentId: string, format: ExportFormat) {
  return invokeCommand<ExportReceipt | null>('export_document', { documentId, format, destination: null });
}
