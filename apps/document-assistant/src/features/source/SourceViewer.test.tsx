import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SourceViewer } from './SourceViewer';

const pages = [
  { pageNumber: 1, imageUrl: 'page-1.png', text: '第一页', status: 'completed', analysis: null, error: null },
  { pageNumber: 2, imageUrl: 'page-2.png', text: '第二页', status: 'failed', analysis: { summary: '图表' }, error: '模型响应无效' },
];

describe('SourceViewer', () => {
  it('shows the selected page and supports previous/next navigation', async () => {
    const onSelectPage = vi.fn();
    render(<SourceViewer pages={pages} selectedPage={2} onSelectPage={onSelectPage} onRetryPage={() => undefined} />);
    expect(screen.getByAltText('第 2 页')).toBeVisible();
    expect(screen.getByText(/模型响应无效/)).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: '上一页' }));
    expect(onSelectPage).toHaveBeenCalledWith(1);
  });

  it('can reveal page text and visual JSON', async () => {
    render(<SourceViewer pages={pages} selectedPage={2} onSelectPage={() => undefined} onRetryPage={() => undefined} />);
    await userEvent.click(screen.getByRole('button', { name: '页面文本' }));
    await userEvent.click(screen.getByRole('button', { name: '视觉 JSON' }));
    expect(screen.getByText('第二页')).toBeVisible();
    expect(screen.getByText(/"summary": "图表"/)).toBeVisible();
  });
});
