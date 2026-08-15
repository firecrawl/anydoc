import { invokeCommand } from '../../lib/desktop/api';

export interface HistoryDocument {
  id: string;
  fileName: string;
  format: string;
  status: string;
  updatedAt: number;
  cacheSize: number;
}

export function listDocumentHistory() {
  return invokeCommand<HistoryDocument[]>('list_documents');
}

export function deleteDocumentCache(documentId: string) {
  return invokeCommand<void>('delete_document_cache', { documentId });
}

export function clearAllDocumentCaches() {
  return invokeCommand<void>('clear_all_document_caches');
}
