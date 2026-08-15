import { describe, expect, it } from 'vitest';
import { createAnyDocClient } from './client';
import type { ConversionResult, ConvertRequest, ConvertResponse } from './types';

class FakeWorker {
  lastMessage?: ConvertRequest;
  onmessage: ((event: MessageEvent<ConvertResponse>) => void) | null = null;

  postMessage(message: ConvertRequest) {
    this.lastMessage = message;
  }

  emit(data: ConvertResponse) {
    this.onmessage?.({ data } as MessageEvent<ConvertResponse>);
  }
}

const sampleResult: ConversionResult = {
  fileName: 'notes.rtf',
  format: 'rtf',
  markdown: '# Notes',
  characterCount: 7,
  elapsedMs: 12,
  document: { blocks: [] },
};

describe('createAnyDocClient', () => {
  it('resolves a conversion response by request id', async () => {
    const worker = new FakeWorker();
    const client = createAnyDocClient(worker as unknown as Worker);
    const promise = client.convert('notes.rtf', new Uint8Array([1, 2]));

    worker.emit({ id: worker.lastMessage!.id, ok: true, result: sampleResult });

    await expect(promise).resolves.toEqual(sampleResult);
  });

  it('preserves the AnyDoc error code and message', async () => {
    const worker = new FakeWorker();
    const client = createAnyDocClient(worker as unknown as Worker);
    const promise = client.convert('locked.docx', new Uint8Array([3, 4]));

    worker.emit({
      id: worker.lastMessage!.id,
      ok: false,
      error: { code: 'encrypted', message: 'document is encrypted' },
    });

    await expect(promise).rejects.toMatchObject({
      code: 'encrypted',
      message: 'document is encrypted',
    });
  });
});
