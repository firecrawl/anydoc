import type { CitedAnswer, EvidenceRef } from '../../lib/analysis/types';
import { invokeCommand } from '../../lib/desktop/api';

export interface AskDocumentRequest {
  documentId: string;
  conversationId: string;
  question: string;
}

export interface ConversationMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  citations: EvidenceRef[];
  createdAt: number;
}

export function askDocument(request: AskDocumentRequest) {
  return invokeCommand<CitedAnswer>('ask_document', { ...request });
}

export function getConversationMessages(conversationId: string) {
  return invokeCommand<ConversationMessage[]>('get_conversation_messages', { conversationId });
}
