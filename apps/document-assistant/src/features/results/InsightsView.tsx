import type { CitedFact, DocumentSummary, EvidenceRef } from '../../lib/analysis/types';
import { AnalysisLimitations } from './AnalysisLimitations';

interface InsightsViewProps {
  summary: DocumentSummary;
  onNavigateToPage: (pageNumber: number) => void;
}

function Citations({ evidence, onNavigateToPage }: { evidence: EvidenceRef[]; onNavigateToPage: (page: number) => void }) {
  return (
    <span className="citations" aria-label="内容出处">
      {evidence.map((item, index) => (
        <button
          key={`${item.pageNumber}-${index}`}
          type="button"
          title={item.excerpt ?? undefined}
          onClick={() => onNavigateToPage(item.pageNumber)}
        >
          第 {item.pageNumber} 页
        </button>
      ))}
    </span>
  );
}

function FactList({ title, items, onNavigateToPage }: { title: string; items: CitedFact[]; onNavigateToPage: (page: number) => void }) {
  if (items.length === 0) return null;
  return (
    <section className="insight-card">
      <h3>{title}</h3>
      <ul>
        {items.map((item, index) => (
          <li key={`${item.text}-${index}`}>
            <span>{item.text}</span>
            <Citations evidence={item.evidence} onNavigateToPage={onNavigateToPage} />
          </li>
        ))}
      </ul>
    </section>
  );
}

export function InsightsView({ summary, onNavigateToPage }: InsightsViewProps) {
  return (
    <article className="insights-view">
      <header className="insights-hero">
        <div>
          <p className="eyebrow">Document intelligence</p>
          <h2>{summary.theme}</h2>
        </div>
        <span className="confidence">置信度 {Math.round(summary.confidence * 100)}%</span>
      </header>
      <p className="executive-summary">{summary.executiveSummary}</p>

      {summary.logicalOutline.length > 0 ? (
        <section className="insight-card">
          <h3>内容脉络</h3>
          <ol className="outline-list">
            {summary.logicalOutline.map((item, index) => (
              <li key={`${item.heading}-${index}`}>
                <button type="button" onClick={() => onNavigateToPage(item.pageStart)}>
                  {item.heading}
                </button>
                <span>{item.summary}</span>
                <small>第 {item.pageStart}{item.pageEnd === item.pageStart ? '' : `–${item.pageEnd}`} 页</small>
              </li>
            ))}
          </ol>
        </section>
      ) : null}

      <div className="insight-grid">
        <FactList title="关键事实" items={summary.keyFacts} onNavigateToPage={onNavigateToPage} />
        <FactList title="风险与疑点" items={summary.risks} onNavigateToPage={onNavigateToPage} />
        <FactList title="行动建议" items={summary.actionItems} onNavigateToPage={onNavigateToPage} />
      </div>
      <AnalysisLimitations limitations={summary.analysisLimitations} />
    </article>
  );
}
