import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { CitedAnswer, EvidenceRef } from '../../lib/analysis/types';
import { askDocument, getConversationMessages, type AskDocumentRequest, type ConversationMessage } from './chatApi';

interface DocumentChatProps {
  documentId: string;
  onNavigateToPage: (pageNumber: number) => void;
  ask?: (request: AskDocumentRequest) => Promise<CitedAnswer>;
}

function nextConversationId() {
  return globalThis.crypto?.randomUUID?.() ?? `chat-${Date.now()}`;
}

export function DocumentChat({ documentId, onNavigateToPage, ask }: DocumentChatProps) {
  const [conversationId, setConversationId] = useState(nextConversationId);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [question, setQuestion] = useState('');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastQuestion = useRef('');
  const askModel = ask ?? askDocument;

  useEffect(() => {
    if (ask) return;
    void getConversationMessages(conversationId).then(setMessages).catch(() => undefined);
  }, [ask, conversationId]);

  const send = async (value = question) => {
    const trimmed = value.trim();
    if (!trimmed || pending) return;
    lastQuestion.current = trimmed;
    setQuestion('');
    setError(null);
    setPending(true);
    const userMessage: ConversationMessage = {
      id: `local-user-${Date.now()}`, role: 'user', content: trimmed, citations: [], createdAt: Date.now(),
    };
    setMessages((current) => [...current, userMessage]);
    try {
      const answer = await askModel({ documentId, conversationId, question: trimmed });
      setMessages((current) => [...current, {
        id: `local-assistant-${Date.now()}`, role: 'assistant', content: answer.answer,
        citations: answer.citations, createdAt: Date.now(),
      }]);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void send();
    }
  };

  const newConversation = () => {
    setConversationId(nextConversationId());
    setMessages([]);
    setQuestion('');
    setError(null);
  };

  return (
    <section className="document-chat" aria-label="文档问答">
      <div className="chat-heading">
        <div>
          <h2>围绕当前文档提问</h2>
          <p>回答仅依据文档证据，并附可核对页码。</p>
        </div>
        <button type="button" onClick={newConversation}>新对话</button>
      </div>
      <div className="chat-messages" aria-live="polite">
        {messages.length === 0 ? <p className="chat-empty">例如：这份文档的核心结论和主要风险是什么？</p> : null}
        {messages.map((message) => (
          <article key={message.id} className={`chat-message chat-message--${message.role}`}>
            <strong>{message.role === 'user' ? '你' : '文档助手'}</strong>
            <p>{message.content}</p>
            {message.citations.length > 0 ? (
              <div className="citations">
                {message.citations.map((citation: EvidenceRef, index) => (
                  <button key={`${citation.pageNumber}-${index}`} type="button" onClick={() => onNavigateToPage(citation.pageNumber)}>
                    第 {citation.pageNumber} 页
                  </button>
                ))}
              </div>
            ) : null}
          </article>
        ))}
        {pending ? <p role="status">正在查找文档证据并生成回答…</p> : null}
        {error ? (
          <div className="chat-error" role="alert">
            <span>{error}</span>
            <button type="button" onClick={() => void send(lastQuestion.current)}>重试</button>
          </div>
        ) : null}
      </div>
      <div className="chat-composer">
        <textarea
          aria-label="向文档提问"
          value={question}
          rows={3}
          placeholder="输入问题，Enter 发送，Shift+Enter 换行"
          onChange={(event) => setQuestion(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        <button type="button" disabled={!question.trim() || pending} onClick={() => void send()}>发送</button>
      </div>
    </section>
  );
}
