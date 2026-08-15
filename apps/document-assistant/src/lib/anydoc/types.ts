export interface ConversionResult {
  fileName: string;
  format: string;
  markdown: string;
  characterCount: number;
  elapsedMs: number;
  document: unknown;
}

export interface ConversionError {
  code?: string;
  message: string;
}

export interface AnyDocClient {
  convert(name: string, bytes: Uint8Array<ArrayBuffer>): Promise<ConversionResult>;
}

export type ConvertRequest = {
  id: string;
  type: 'convert';
  name: string;
  bytes: Uint8Array<ArrayBuffer>;
};

export type ConvertResponse =
  | { id: string; ok: true; result: ConversionResult }
  | { id: string; ok: false; error: ConversionError };
