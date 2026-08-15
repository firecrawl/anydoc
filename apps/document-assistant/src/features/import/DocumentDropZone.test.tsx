import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DocumentDropZone } from './DocumentDropZone';

describe('DocumentDropZone', () => {
  it('passes the selected document to onFile', async () => {
    const onFile = vi.fn();
    render(<DocumentDropZone onFile={onFile} />);
    const file = new File(['demo'], 'demo.docx');

    await userEvent.upload(screen.getByLabelText(/选择文档/i), file);

    expect(onFile).toHaveBeenCalledWith(file);
  });
});
