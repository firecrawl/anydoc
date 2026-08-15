import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ResultTabs } from './ResultTabs';

describe('ResultTabs', () => {
  it('changes result views', async () => {
    const onChange = vi.fn();
    render(<ResultTabs active="markdown" onChange={onChange} />);

    await userEvent.click(screen.getByRole('tab', { name: '智能解读' }));

    expect(onChange).toHaveBeenCalledWith('insights');
  });

  it('exposes all result views as accessible tabs', () => {
    render(<ResultTabs active="markdown" onChange={() => undefined} />);

    expect(screen.getAllByRole('tab')).toHaveLength(5);
    expect(screen.getByRole('tab', { name: 'Markdown' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });
});
