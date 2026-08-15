import { useRef, type ChangeEvent, type DragEvent, type KeyboardEvent } from 'react';

interface DocumentDropZoneProps {
  onFile: (file: File) => void;
}

const ACCEPTED_DOCUMENTS = [
  '.doc',
  '.docx',
  '.ppt',
  '.pptx',
  '.pdf',
  '.rtf',
  '.odt',
  '.odp',
  '.xls',
  '.xlsx',
  '.csv',
  '.epub',
].join(',');

export function DocumentDropZone({ onFile }: DocumentDropZoneProps) {
  const inputRef = useRef<HTMLInputElement>(null);

  const acceptFirstFile = (files: FileList | null) => {
    const [file] = files ? Array.from(files) : [];
    if (file) onFile(file);
  };

  const handleChange = (event: ChangeEvent<HTMLInputElement>) => {
    acceptFirstFile(event.currentTarget.files);
    event.currentTarget.value = '';
  };

  const handleDrop = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    acceptFirstFile(event.dataTransfer.files);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      inputRef.current?.click();
    }
  };

  return (
    <div
      className="drop-zone"
      role="button"
      tabIndex={0}
      onClick={() => inputRef.current?.click()}
      onKeyDown={handleKeyDown}
      onDragOver={(event) => event.preventDefault()}
      onDrop={handleDrop}
    >
      <input
        ref={inputRef}
        className="visually-hidden"
        type="file"
        accept={ACCEPTED_DOCUMENTS}
        aria-label="选择文档"
        onChange={handleChange}
        onClick={(event) => event.stopPropagation()}
      />
      <span className="drop-icon" aria-hidden="true">
        ↑
      </span>
      <div>
        <strong>拖放文档到这里</strong>
        <span>或点击选择文件</span>
      </div>
      <p>支持 Word、PowerPoint、PDF、表格、RTF、EPUB 等格式</p>
    </div>
  );
}
