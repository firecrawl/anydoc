use std::{
    fs::File,
    io::{BufReader, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentIdentity {
    pub id: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePaths {
    pub root: PathBuf,
    pub pages: PathBuf,
    pub assets: PathBuf,
    pub markdown: PathBuf,
}

pub fn compute_document_id(path: &Path) -> Result<DocumentIdentity> {
    let file = File::open(path)
        .with_context(|| format!("open document for hashing: {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let sha256 = format!("{:x}", hasher.finalize());
    Ok(DocumentIdentity {
        id: sha256.clone(),
        sha256,
        size,
    })
}

pub fn cache_paths(app_data: &Path, document_id: &str) -> Result<CachePaths> {
    validate_document_id(document_id)?;
    let root = app_data.join("documents").join(document_id);
    let pages = root.join("pages");
    let assets = root.join("assets");

    std::fs::create_dir_all(&pages)
        .with_context(|| format!("create page cache {}", pages.display()))?;
    std::fs::create_dir_all(&assets)
        .with_context(|| format!("create asset cache {}", assets.display()))?;

    if !root.starts_with(app_data) {
        bail!("cache path escaped the application data directory");
    }

    Ok(CachePaths {
        markdown: root.join("document.md"),
        root,
        pages,
        assets,
    })
}

pub fn remove_document_cache(app_data: &Path, document_id: &str) -> Result<()> {
    let paths = cache_paths(app_data, document_id)?;
    if paths.root.exists() {
        std::fs::remove_dir_all(&paths.root)
            .with_context(|| format!("remove document cache {}", paths.root.display()))?;
    }
    Ok(())
}

pub fn directory_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total = total.saturating_add(if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        });
    }
    Ok(total)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("derived file has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("persist derived file {}", path.display()))?;
    Ok(())
}

fn validate_document_id(document_id: &str) -> Result<()> {
    let safe_characters = document_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    let has_path_components = Path::new(document_id).components().any(|component| {
        !matches!(component, Component::Normal(_))
    });
    if document_id.is_empty() || !safe_characters || has_path_components {
        bail!("invalid document id");
    }
    Ok(())
}
