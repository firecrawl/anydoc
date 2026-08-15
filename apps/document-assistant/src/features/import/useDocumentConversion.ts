import { useCallback, useState } from 'react';
import { getAnyDocClient } from '../../lib/anydoc/client';
import type {
  AnyDocClient,
  ConversionError,
  ConversionResult,
} from '../../lib/anydoc/types';

export type ConversionState =
  | { status: 'idle' }
  | { status: 'reading'; fileName: string }
  | { status: 'converting'; fileName: string }
  | { status: 'completed'; result: ConversionResult }
  | { status: 'failed'; error: ConversionError };

function toConversionError(error: unknown): ConversionError {
  if (error instanceof Error) {
    return {
      code:
        'code' in error && typeof error.code === 'string' ? error.code : undefined,
      message: error.message,
    };
  }
  return { message: String(error) };
}

export function useDocumentConversion(client?: AnyDocClient) {
  const [state, setState] = useState<ConversionState>({ status: 'idle' });

  const convert = useCallback(
    async (file: File) => {
      try {
        setState({ status: 'reading', fileName: file.name });
        const buffer = await file.arrayBuffer();
        setState({ status: 'converting', fileName: file.name });
        const result = await (client ?? getAnyDocClient()).convert(
          file.name,
          new Uint8Array(buffer),
        );
        setState({ status: 'completed', result });
      } catch (error) {
        setState({ status: 'failed', error: toConversionError(error) });
      }
    },
    [client],
  );

  const reset = useCallback(() => setState({ status: 'idle' }), []);

  return { state, convert, reset };
}
