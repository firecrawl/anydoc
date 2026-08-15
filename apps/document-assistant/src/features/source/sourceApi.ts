import { convertFileSrc } from '@tauri-apps/api/core';
import { invokeCommand } from '../../lib/desktop/api';
import type { SourcePage } from './SourceViewer';

interface SourcePageData {
  pageNumber: number;
  imagePath: string | null;
  text: string | null;
  status: string;
  analysis: unknown | null;
  error: string | null;
}

export async function getDocumentPages(documentId: string): Promise<SourcePage[]> {
  const pages = await invokeCommand<SourcePageData[]>('get_document_pages', { documentId });
  return pages.map(({ imagePath, ...page }) => ({
    ...page,
    imageUrl: imagePath ? convertFileSrc(imagePath) : null,
  }));
}
