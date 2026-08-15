export interface EvidenceRef {
  pageNumber: number;
  excerpt: string | null;
}

export interface UncertainItem {
  description: string;
  reason: string;
  pageNumber: number | null;
}

export interface VisualElement {
  kind: string;
  description: string;
  evidence: EvidenceRef[];
}

export interface LogicalRelation {
  source: string;
  target: string;
  relation: string;
  evidence: EvidenceRef[];
}

export interface PageAnalysis {
  pageNumber: number;
  title: string | null;
  summary: string;
  visualElements: VisualElement[];
  logicalRelations: LogicalRelation[];
  keyFacts: string[];
  uncertainItems: UncertainItem[];
  confidence: number;
}

export interface CitedFact {
  text: string;
  evidence: EvidenceRef[];
}

export interface OutlineItem {
  heading: string;
  summary: string;
  pageStart: number;
  pageEnd: number;
}

export interface DocumentSummary {
  schemaVersion: number;
  theme: string;
  executiveSummary: string;
  logicalOutline: OutlineItem[];
  keyFacts: CitedFact[];
  risks: CitedFact[];
  actionItems: CitedFact[];
  analysisLimitations: string[];
  confidence: number;
}

export interface CitedAnswer {
  answer: string;
  citations: EvidenceRef[];
}
