import { invokeCommand } from '../desktop/api';
import type { DocumentSummary } from './types';

export interface AnalyzeDocumentRequest {
  documentId: string;
  visionProfileId: string | null;
  textProfileId: string;
  confirmRemoteProcessing: boolean;
}

export function analyzeDocument(request: AnalyzeDocumentRequest) {
  return invokeCommand<DocumentSummary>('analyze_document', { ...request });
}

export function getDocumentAnalysis(documentId: string) {
  return invokeCommand<DocumentSummary | null>('get_document_analysis', { documentId });
}
