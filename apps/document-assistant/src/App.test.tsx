import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App', () => {
  it('renders the AnyDoc Assistant shell', () => {
    render(<App />);

    expect(
      screen.getByRole('heading', { name: /Any document in/i }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: /模型设置/i })).toBeVisible();
  });
});
