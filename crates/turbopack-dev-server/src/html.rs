use std::sync::Arc;

use anyhow::{anyhow, Result};
use mime_guess::mime::TEXT_HTML_UTF_8;
use turbo_tasks::{debug::ValueDebug, primitives::StringVc, ValueToString};
use turbo_tasks_fs::{embed_file, File, FileContent, FileContentVc, FileSystemPathVc};
use turbopack_core::{
    asset::{Asset, AssetVc},
    chunk::{ChunkGroupVc, ChunkReferenceVc},
    reference::{AssetReferencesVc, SingleAssetReferenceVc},
    version::{Update, UpdateVc, Version, VersionVc, VersionedContent, VersionedContentVc},
};
use turbopack_hash::{encode_hex, Xxh3Hash64Hasher};

/// The HTML entry point of the dev server.
///
/// Generates an HTML page that includes the ES and CSS chunks.
#[turbo_tasks::value(shared)]
pub struct DevHtmlAsset {
    path: FileSystemPathVc,
    chunk_groups: Vec<ChunkGroupVc>,
}

#[turbo_tasks::value_impl]
impl Asset for DevHtmlAsset {
    #[turbo_tasks::function]
    fn path(&self) -> FileSystemPathVc {
        self.path
    }

    #[turbo_tasks::function]
    fn content(self_vc: DevHtmlAssetVc) -> FileContentVc {
        self_vc.html_content().content()
    }

    #[turbo_tasks::function]
    async fn references(self_vc: DevHtmlAssetVc) -> Result<AssetReferencesVc> {
        let this = self_vc.await?;
        let mut references = Vec::new();
        for chunk_group in &this.chunk_groups {
            let chunks = chunk_group.chunks().await?;
            for chunk in chunks.iter() {
                references.push(ChunkReferenceVc::new(*chunk).into());
            }
        }
        references.push(self_vc.html_runtime_reference().into());
        Ok(AssetReferencesVc::cell(references))
    }

    #[turbo_tasks::function]
    fn versioned_content(self_vc: DevHtmlAssetVc) -> VersionedContentVc {
        self_vc.html_content().into()
    }
}

#[turbo_tasks::value_impl]
impl DevHtmlAssetVc {
    #[turbo_tasks::function]
    async fn html_runtime_reference(self) -> Result<SingleAssetReferenceVc> {
        let path = self.await?.path.parent().join("_turbopack/html-runtime.js");
        Ok(SingleAssetReferenceVc::new(
            HtmlRuntimeAssetVc::new(path).into(),
            StringVc::cell(format!("html-runtime {}", path.await?)),
        ))
    }
}

/// The HTML runtime asset.
#[turbo_tasks::value]
struct HtmlRuntimeAsset {
    path: FileSystemPathVc,
}

#[turbo_tasks::value_impl]
impl Asset for HtmlRuntimeAsset {
    #[turbo_tasks::function]
    fn path(&self) -> FileSystemPathVc {
        self.path
    }

    #[turbo_tasks::function]
    fn content(&self) -> FileContentVc {
        embed_file!("html-runtime.js")
    }

    #[turbo_tasks::function]
    fn references(&self) -> AssetReferencesVc {
        AssetReferencesVc::empty()
    }
}

#[turbo_tasks::value_impl]
impl HtmlRuntimeAssetVc {
    #[turbo_tasks::function]
    fn new(path: FileSystemPathVc) -> Self {
        Self::cell(HtmlRuntimeAsset { path })
    }
}

impl DevHtmlAsset {
    /// Create a new dev HTML asset.
    pub fn new(path: FileSystemPathVc, chunk_groups: Vec<ChunkGroupVc>) -> Self {
        DevHtmlAsset { path, chunk_groups }
    }
}

#[turbo_tasks::value_impl]
impl DevHtmlAssetVc {
    #[turbo_tasks::function]
    async fn html_content(self) -> Result<DevHtmlAssetContentVc> {
        let this = self.await?;
        let context_path = this.path.parent().await?;

        let mut chunk_paths = vec![];
        for chunk_group in &this.chunk_groups {
            for chunk in chunk_group.chunks().await?.iter() {
                let chunk_id = chunk.path().to_string().await?;
                let chunk_path = &*chunk.path().await?;
                if let Some(relative_path) = context_path.get_relative_path_to(chunk_path) {
                    chunk_paths.push((relative_path, chunk_id.clone()));
                }
            }
        }

        let html_runtime_reference = &*self.html_runtime_reference().asset().path().await?;
        let html_runtime_path = context_path
            .get_relative_path_to(html_runtime_reference)
            .ok_or_else(|| anyhow!("html runtime path is not relative to context path"))?;

        Ok(DevHtmlAssetContent::new(chunk_paths, html_runtime_path).cell())
    }
}

#[turbo_tasks::value]
struct DevHtmlAssetContent {
    chunk_paths: Arc<Vec<(String, String)>>,
    html_runtime_path: String,
}

impl DevHtmlAssetContent {
    pub fn new(chunk_paths: Vec<(String, String)>, html_runtime_path: String) -> Self {
        DevHtmlAssetContent {
            chunk_paths: Arc::new(chunk_paths),
            html_runtime_path,
        }
    }
}

#[turbo_tasks::value_impl]
impl DevHtmlAssetContentVc {
    #[turbo_tasks::function]
    async fn content(self) -> Result<FileContentVc> {
        let this = self.await?;

        let mut scripts = Vec::new();
        let mut stylesheets = Vec::new();

        // The HTML runtime MUST be the first script to be loaded.
        scripts.push(format!(
            "<script src=\"{}\"></script>",
            this.html_runtime_path
        ));

        for (relative_path, chunk_id) in &*this.chunk_paths {
            if relative_path.ends_with(".js") {
                scripts.push(format!("<script src=\"{}\"></script>", relative_path));
            } else if relative_path.ends_with(".css") {
                stylesheets.push(format!(
                    "<link data-turbopack-chunk-id=\"{}\" rel=\"stylesheet\" href=\"{}\">",
                    chunk_id, relative_path
                ));
            } else {
                return Err(anyhow!("chunk with unknown asset type: {}", relative_path));
            }
        }

        let html = format!(
            "<!DOCTYPE html>\n<html>\n<head>\n{}\n</head>\n<body>\n<div \
             id=root></div>\n{}\n</body>\n</html>",
            stylesheets.join("\n"),
            scripts.join("\n"),
        );

        Ok(FileContent::Content(File::from_source(html).with_content_type(TEXT_HTML_UTF_8)).into())
    }

    #[turbo_tasks::function]
    async fn version(self) -> Result<DevHtmlAssetVersionVc> {
        let this = self.await?;
        Ok(DevHtmlAssetVersion {
            chunk_paths: Arc::clone(&this.chunk_paths),
        }
        .cell())
    }
}

#[turbo_tasks::value_impl]
impl VersionedContent for DevHtmlAssetContent {
    #[turbo_tasks::function]
    fn content(self_vc: DevHtmlAssetContentVc) -> FileContentVc {
        self_vc.content()
    }

    #[turbo_tasks::function]
    fn version(self_vc: DevHtmlAssetContentVc) -> VersionVc {
        self_vc.version().into()
    }

    #[turbo_tasks::function]
    async fn update(self_vc: DevHtmlAssetContentVc, from_version: VersionVc) -> Result<UpdateVc> {
        let from_version = DevHtmlAssetVersionVc::resolve_from(from_version)
            .await?
            .expect("version must be an `DevHtmlAssetVersionVc`");
        let to_version = self_vc.version();

        let to = to_version.await?;
        let from = from_version.await?;

        if to.chunk_paths == from.chunk_paths {
            return Ok(Update::None.into());
        }

        Err(anyhow!(
            "cannot update `DevHtmlAssetContentVc` from version {:?} to version {:?}: the \
             versions contain different chunks, which is not yet supported",
            from_version.dbg().await?,
            to_version.dbg().await?,
        ))
    }
}

#[turbo_tasks::value]
struct DevHtmlAssetVersion {
    chunk_paths: Arc<Vec<(String, String)>>,
}

#[turbo_tasks::value_impl]
impl Version for DevHtmlAssetVersion {
    #[turbo_tasks::function]
    async fn id(&self) -> Result<StringVc> {
        let mut hasher = Xxh3Hash64Hasher::new();
        for (relative_path, chunk_id) in &*self.chunk_paths {
            hasher.write(relative_path.as_bytes());
            hasher.write(chunk_id.as_bytes());
        }
        let hash = hasher.finish();
        let hex_hash = encode_hex(hash);
        Ok(StringVc::cell(hex_hash))
    }
}
