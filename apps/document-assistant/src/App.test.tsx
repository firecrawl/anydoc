import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { AnyDocClient } from './lib/anydoc/types';
import { App } from './App';

describe('App', () => {
  it('renders the AnyDoc Assistant shell', () => {
    render(<App />);

    expect(
      screen.getByRole('heading', { name: /Any document in/i }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: /模型设置/i })).toBeVisible();
  });

  it('shows the locally converted markdown after document upload', async () => {
    const client: AnyDocClient = {
      convert: async (fileName) => ({
        fileName,
        format: 'rtf',
        markdown: '# Local conversion',
        characterCount: 18,
        elapsedMs: 8,
        document: { blocks: [] },
      }),
    };
    render(<App anyDocClient={client} />);
    const uploadedFile = new File(['{\\rtf1 Local conversion}'], 'sample.rtf');
    Object.defineProperty(uploadedFile, 'arrayBuffer', {
      value: async () => new TextEncoder().encode('{\\rtf1 Local conversion}').buffer,
    });

    await userEvent.upload(
      screen.getByLabelText('选择文档'),
      uploadedFile,
    );

    expect(await screen.findByText('# Local conversion')).toBeVisible();
  });
});
