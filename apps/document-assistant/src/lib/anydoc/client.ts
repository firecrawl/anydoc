import type {
  AnyDocClient,
  ConversionError,
  ConversionResult,
  ConvertRequest,
  ConvertResponse,
} from './types';

interface PendingConversion {
  resolve: (result: ConversionResult) => void;
  reject: (error: Error & { code?: string }) => void;
}

let requestSequence = 0;
let browserClient: AnyDocClient | undefined;

function conversionError(error: ConversionError) {
  return Object.assign(new Error(error.message), { code: error.code });
}

export function createAnyDocClient(worker: Worker): AnyDocClient {
  const pending = new Map<string, PendingConversion>();

  worker.onmessage = (event: MessageEvent<ConvertResponse>) => {
    const response = event.data;
    const conversion = pending.get(response.id);
    if (!conversion) return;

    pending.delete(response.id);
    if (response.ok) {
      conversion.resolve(response.result);
    } else {
      conversion.reject(conversionError(response.error));
    }
  };

  worker.onerror = (event) => {
    const error = new Error(event.message || 'AnyDoc worker failed');
    pending.forEach(({ reject }) => reject(error));
    pending.clear();
  };

  return {
    convert(name, bytes) {
      const id = `conversion-${Date.now()}-${++requestSequence}`;
      const request: ConvertRequest = { id, type: 'convert', name, bytes };

      return new Promise<ConversionResult>((resolve, reject) => {
        pending.set(id, { resolve, reject });
        worker.postMessage(request, [bytes.buffer]);
      });
    },
  };
}

export function getAnyDocClient(): AnyDocClient {
  browserClient ??= createAnyDocClient(
    new Worker(new URL('./anydoc.worker.ts', import.meta.url), { type: 'module' }),
  );
  return browserClient;
}
