import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { AnyDocClient, ConversionResult } from '../../lib/anydoc/types';
import { useDocumentConversion } from './useDocumentConversion';

const converted: ConversionResult = {
  fileName: 'notes.rtf',
  format: 'rtf',
  markdown: '# Notes',
  characterCount: 7,
  elapsedMs: 12,
  document: { blocks: [] },
};

const file = {
  name: 'notes.rtf',
  arrayBuffer: async () => new Uint8Array([1, 2]).buffer,
} as File;

describe('useDocumentConversion', () => {
  it('publishes the completed conversion result', async () => {
    const client: AnyDocClient = {
      convert: async () => converted,
    };
    const { result } = renderHook(() => useDocumentConversion(client));

    await act(async () => result.current.convert(file));

    expect(result.current.state).toEqual({
      status: 'completed',
      result: converted,
    });
  });

  it('keeps the parser error details in the failed state', async () => {
    const parserError = Object.assign(new Error('document is encrypted'), {
      code: 'encrypted',
    });
    const client: AnyDocClient = {
      convert: async () => {
        throw parserError;
      },
    };
    const { result } = renderHook(() => useDocumentConversion(client));

    await act(async () => result.current.convert(file));

    expect(result.current.state).toEqual({
      status: 'failed',
      error: { code: 'encrypted', message: 'document is encrypted' },
    });
  });
});
