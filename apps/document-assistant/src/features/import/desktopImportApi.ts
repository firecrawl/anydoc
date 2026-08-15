import { invokeCommand } from '../../lib/desktop/api';
import type { HistoryDocument } from '../history/historyApi';

export interface SelectedDocument {
  document: HistoryDocument;
  bytes: number[];
}

export function pickAndRegisterDocument() {
  return invokeCommand<SelectedDocument | null>('pick_and_register_document');
}

export function saveDocumentMarkdown(documentId: string, markdown: string) {
  return invokeCommand<void>('save_document_markdown', { documentId, markdown });
}
