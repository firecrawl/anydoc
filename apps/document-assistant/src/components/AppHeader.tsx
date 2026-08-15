interface AppHeaderProps {
  onOpenSettings: () => void;
}

export function AppHeader({ onOpenSettings }: AppHeaderProps) {
  return (
    <header className="app-header">
      <a className="brand" href="#top" aria-label="AnyDoc Assistant 首页">
        <img src="/assets/anydoc/logo.svg" alt="" />
        <span>anydoc</span>
        <span className="brand-product">assistant</span>
      </a>
      <nav className="header-actions" aria-label="应用操作">
        <a
          className="text-link"
          href="https://github.com/firecrawl/anydoc"
          target="_blank"
          rel="noreferrer"
        >
          开源项目
        </a>
        <button className="settings-button" type="button" onClick={onOpenSettings}>
          <span aria-hidden="true">⚙</span>
          模型设置
        </button>
      </nav>
    </header>
  );
}
