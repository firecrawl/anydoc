/// <reference lib="webworker" />

import init, {
  formatFromBytes,
  formatFromPath,
  toDocument,
  toMarkdownBytes,
  type Format,
} from '@firecrawl/anydoc-wasm';
import type { ConversionError, ConvertRequest, ConvertResponse } from './types';

const workerScope = self as unknown as DedicatedWorkerGlobalScope;
let initializePromise: Promise<unknown> | undefined;

function ensureInitialized() {
  initializePromise ??= init();
  return initializePromise;
}

function serializeError(error: unknown): ConversionError {
  if (error instanceof Error) {
    return {
      code:
        'code' in error && typeof error.code === 'string' ? error.code : undefined,
      message: error.message,
    };
  }
  return { message: String(error) };
}

workerScope.onmessage = async (event: MessageEvent<ConvertRequest>) => {
  const request = event.data;
  if (request.type !== 'convert') return;

  try {
    await ensureInitialized();
    const startedAt = performance.now();
    const detectedFormat =
      formatFromBytes(request.bytes) ?? formatFromPath(request.name);
    const markdown = toMarkdownBytes(request.bytes, detectedFormat);
    const document =
      detectedFormat === ('pdf' as Format)
        ? null
        : toDocument(request.bytes, detectedFormat);
    const result = {
      fileName: request.name,
      format: detectedFormat ?? 'unknown',
      markdown,
      characterCount: markdown.length,
      elapsedMs: Math.round((performance.now() - startedAt) * 10) / 10,
      document,
    };
    const response: ConvertResponse = { id: request.id, ok: true, result };
    workerScope.postMessage(response);
  } catch (error) {
    const response: ConvertResponse = {
      id: request.id,
      ok: false,
      error: serializeError(error),
    };
    workerScope.postMessage(response);
  }
};
