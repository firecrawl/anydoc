import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DocumentChat } from './DocumentChat';

describe('DocumentChat', () => {
  it('sends with Enter, preserves Shift+Enter, and navigates citations', async () => {
    const ask = vi.fn().mockResolvedValue({
      answer: '增长来自西南区域。', grounded: true,
      citations: [{ pageNumber: 2, excerpt: '西南区域' }],
    });
    const onNavigateToPage = vi.fn();
    render(<DocumentChat documentId="doc-1" ask={ask} onNavigateToPage={onNavigateToPage} />);
    const input = screen.getByLabelText('向文档提问');

    await userEvent.type(input, '增长{shift>}{enter}{/shift}来源？');
    expect(input).toHaveValue('增长\n来源？');
    await userEvent.type(input, '{enter}');

    expect(ask).toHaveBeenCalledWith(expect.objectContaining({ question: '增长\n来源？' }));
    expect(await screen.findByText('增长来自西南区域。')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: '第 2 页' }));
    expect(onNavigateToPage).toHaveBeenCalledWith(2);
  });
});
