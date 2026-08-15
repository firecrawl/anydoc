import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { DocumentSummary } from '../../lib/analysis/types';
import { InsightsView } from './InsightsView';

const summary: DocumentSummary = {
  schemaVersion: 1,
  theme: '季度经营复盘',
  executiveSummary: '收入增长，但应收账款风险上升。',
  logicalOutline: [{ heading: '经营结果', summary: '核心指标', pageStart: 1, pageEnd: 2 }],
  keyFacts: [{ text: '收入同比增长 12%', evidence: [{ pageNumber: 2, excerpt: '同比 +12%' }] }],
  risks: [{ text: '回款周期变长', evidence: [{ pageNumber: 3, excerpt: null }] }],
  actionItems: [{ text: '跟进重点客户回款', evidence: [{ pageNumber: 3, excerpt: null }] }],
  analysisLimitations: ['视觉内容未分析，仅依据提取文本生成结果。'],
  confidence: 0.82,
};

describe('InsightsView', () => {
  it('shows insights and visual-analysis limitations', () => {
    render(<InsightsView summary={summary} onNavigateToPage={() => undefined} />);

    expect(screen.getByRole('heading', { name: '季度经营复盘' })).toBeVisible();
    expect(screen.getByText(/视觉内容未分析/)).toBeVisible();
    expect(screen.getByText('收入同比增长 12%')).toBeVisible();
  });

  it('navigates to the cited source page', async () => {
    const onNavigateToPage = vi.fn();
    render(<InsightsView summary={summary} onNavigateToPage={onNavigateToPage} />);

    await userEvent.click(screen.getAllByRole('button', { name: '第 2 页' })[0]);
    expect(onNavigateToPage).toHaveBeenCalledWith(2);
  });
});
