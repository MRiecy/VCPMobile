use super::*;
use crate::vcp_modules::media_processor::image_extractor::{derive_image, DerivedImage};
use tokio::io::AsyncWriteExt;

#[cfg(any(target_os = "android", test))]
mod avif;

const TEXT_PREVIEW_BYTES: u64 = 128 * 1024;
const DISPLAY_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
#[cfg(any(target_os = "android", test))]
const AVIF_DISPLAY_SOURCE_BYTES: u64 = 24 * 1024 * 1024;
const DISPLAY_IMAGE_PIXELS: u64 = 4 * 1024 * 1024;
const REMOTE_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    text: String,
    truncated: bool,
    total_bytes: u64,
    decoding: String,
}

#[tauri::command]
pub async fn read_attachment_text_preview(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
) -> Result<TextPreview, String> {
    let file = resolve_attachment_cas_file(&app_handle, &db_state.pool, &hash).await?;
    let input = tokio::fs::File::open(file.path)
        .await
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(TEXT_PREVIEW_BYTES as usize + 1);
    input
        .take(TEXT_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() as u64 > TEXT_PREVIEW_BYTES;
    bytes.truncate(TEXT_PREVIEW_BYTES as usize);
    let (text, decoding) = decode_preview_with_encoding(&bytes, !truncated);
    Ok(TextPreview {
        text,
        truncated,
        total_bytes: file.size_bytes,
        decoding,
    })
}

fn decode_preview_with_encoding(bytes: &[u8], complete: bool) -> (String, String) {
    let (encoding, skip) = encoding_rs::Encoding::for_bom(bytes).unwrap_or_else(|| {
        let mut detector = chardetng::EncodingDetector::new();
        detector.feed(bytes, complete);
        (detector.guess(None, true), 0)
    });
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let mut text = String::with_capacity(bytes.len() * 4 + 16);
    // Incomplete final UTF-8/UTF-16 sequences stay in the decoder at a truncated boundary.
    let _ = decoder.decode_to_string(&bytes[skip..], &mut text, complete);
    (text, encoding.name().to_string())
}

fn image_work_root<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(get_cache_root_dir(app)?.join("image-work"))
}
fn export_root<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(get_cache_root_dir(app)?.join("temporary-exports"))
}
fn rendered_root<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(get_cache_root_dir(app)?.join("rendered-input"))
}

async fn display_image(
    app: &AppHandle,
    source: PathBuf,
    max_dimension: Option<u32>,
) -> Result<DerivedImage, String> {
    let output = image_work_root(app)?;
    tokio::fs::create_dir_all(&output)
        .await
        .map_err(|e| e.to_string())?;
    let stem = output.join(uuid::Uuid::new_v4().to_string());
    let app = app.clone();
    let dimension = max_dimension.unwrap_or(2048).clamp(256, 2048);
    tokio::task::spawn_blocking(move || {
        derive_image(
            &app,
            &source,
            &stem,
            dimension,
            DISPLAY_IMAGE_PIXELS,
            DISPLAY_IMAGE_BYTES,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Display consumers release only images they own. A bounded AVIF original is
/// borrowed from the CAS and must never enter the temporary-file deletion path.
#[derive(Serialize)]
pub struct AttachmentImage {
    #[serde(flatten)]
    image: DerivedImage,
    owned: bool,
}

#[cfg(any(target_os = "android", test))]
fn borrow_avif_display(file: &AttachmentCasFile) -> Result<AttachmentImage, String> {
    let metadata = avif::probe(&file.path, AVIF_DISPLAY_SOURCE_BYTES, DISPLAY_IMAGE_PIXELS)
        .map_err(|error| format!("此 AVIF 无法在显示预算内预览：{error}。请使用系统预览。"))?;
    if metadata.size != file.size_bytes {
        return Err("AVIF 原件大小发生变化，请重新导入".into());
    }
    Ok(AttachmentImage {
        image: DerivedImage {
            path: file.path.to_string_lossy().into_owned(),
            mime: "image/avif".into(),
            size: metadata.size,
            width: metadata.width,
            height: metadata.height,
        },
        owned: false,
    })
}

#[tauri::command]
pub async fn get_attachment_image(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
    purpose: String,
    max_dimension: Option<u32>,
) -> Result<AttachmentImage, String> {
    if !matches!(purpose.as_str(), "display" | "avatar") {
        return Err("无效的图片用途".into());
    }
    let file = resolve_attachment_cas_file(&app_handle, &db_state.pool, &hash).await?;
    match display_image(&app_handle, file.path.clone(), max_dimension).await {
        Ok(image) => Ok(AttachmentImage { image, owned: true }),
        Err(error) => {
            #[cfg(target_os = "android")]
            if purpose == "display"
                && file.mime_type == "image/avif"
                && error.contains("image_decode:")
            {
                // WebView's AVIF support is independent of the OS decoder. The
                // encoded-byte and source-pixel budgets are separate: a small
                // compressed file is not evidence of a small decoded surface.
                return tokio::task::spawn_blocking(move || borrow_avif_display(&file))
                    .await
                    .map_err(|error| format!("AVIF 显示准备失败：{error}"))?;
            }
            Err(error)
        }
    }
}

async fn remove_owned_file(root: PathBuf, path: String) -> Result<(), String> {
    if !tokio::fs::try_exists(&path)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    let canonical = canonical_staged_file(&root, Path::new(&path), "临时资源")?;
    tokio::fs::remove_file(canonical)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn release_attachment_image(app_handle: AppHandle, path: String) -> Result<(), String> {
    remove_owned_file(image_work_root(&app_handle)?, path).await
}

#[derive(Serialize)]
pub struct TemporaryExport {
    path: String,
    size: u64,
}

#[tauri::command]
pub async fn store_temporary_export(
    app_handle: AppHandle,
    file_bytes: Vec<u8>,
    original_name: String,
    operation_id: Option<String>,
) -> Result<TemporaryExport, String> {
    if file_bytes.len() > STORE_FILE_IPC_MAX_BYTES {
        return Err("导出数据超过 8 MiB，请缩小输出".into());
    }
    let size = file_bytes.len() as u64;
    let root = export_root(&app_handle)?;
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let filename = safe_storage_extension(&original_name)
        .map(|ext| format!("{id}.{ext}"))
        .unwrap_or(id);
    let path = root.join(filename);
    let operation = operation_id
        .map(|id| operations::acquire(Some(id)).map(|(_, op)| op))
        .transpose()?;
    let _permit = store_file_semaphore()
        .acquire_owned()
        .await
        .map_err(|_| "文件存储执行器已关闭".to_string())?;
    let result = async {
        if let Some(op) = &operation {
            op.check()?;
        }
        tokio::fs::write(&path, file_bytes)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(op) = &operation {
            op.check()?;
        }
        Ok(TemporaryExport {
            path: path.to_string_lossy().into_owned(),
            size,
        })
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

#[tauri::command]
pub async fn release_temporary_export(app_handle: AppHandle, path: String) -> Result<(), String> {
    remove_owned_file(export_root(&app_handle)?, path).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedImage {
    path: String,
    display_path: String,
    mime: String,
    size: u64,
    width: u32,
    height: u32,
    display_mime: String,
    display_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    svg_markup: Option<String>,
}

// This only identifies a bounded SVG candidate. The renderer's XML parser validates the
// complete document before using its root as an SVG export source.
fn is_svg_candidate(markup: &str) -> bool {
    let mut root = markup.trim_start_matches('\u{feff}').trim_start();
    loop {
        let terminator = if root.starts_with("<?") {
            "?>"
        } else if root.starts_with("<!--") {
            "-->"
        } else {
            break;
        };
        let Some(end) = root.find(terminator) else {
            return false;
        };
        root = root[end + terminator.len()..].trim_start();
    }
    root.strip_prefix("<svg").is_some_and(|tail| {
        tail.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
    })
}

fn local_image_path(source: &str) -> Result<Option<PathBuf>, String> {
    if source.starts_with("asset://")
        || source.starts_with("http://asset.localhost/")
        || source.starts_with("https://asset.localhost/")
        || source.starts_with("file://")
    {
        let url = reqwest::Url::parse(source).map_err(|e| e.to_string())?;
        let decoded = percent_encoding::percent_decode_str(url.path())
            .decode_utf8()
            .map_err(|e| e.to_string())?;
        return Ok(Some(PathBuf::from(decoded.as_ref())));
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return Ok(None);
    }
    if source.starts_with("data:") || source.starts_with("blob:") {
        return Err("内存图片请先有界存入导出暂存".into());
    }
    Ok(Some(PathBuf::from(source)))
}

#[tauri::command]
pub async fn prepare_rendered_image(
    app_handle: AppHandle,
    source: String,
) -> Result<RenderedImage, String> {
    let mut downloaded = false;
    let path = if let Some(path) = local_image_path(&source)? {
        ensure_safe_path(&app_handle, &path)?;
        path
    } else {
        let root = rendered_root(&app_handle)?;
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| e.to_string())?;
        let path = root.join(uuid::Uuid::new_v4().to_string());
        let download = async {
            let client = crate::vcp_modules::infra::http_clients::client(
                crate::vcp_modules::infra::http_clients::HttpProfile::ChatStream,
            );
            let mut response = client
                .get(source)
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?;
            if response
                .content_length()
                .is_some_and(|size| size > REMOTE_IMAGE_BYTES)
            {
                return Err("图片超过 32 MiB".to_string());
            }
            let mut file = tokio::fs::File::create(&path)
                .await
                .map_err(|e| e.to_string())?;
            let mut size = 0u64;
            while let Some(bytes) = response.chunk().await.map_err(|e| e.to_string())? {
                size = size.checked_add(bytes.len() as u64).ok_or("图片大小溢出")?;
                if size > REMOTE_IMAGE_BYTES {
                    return Err("图片超过 32 MiB".into());
                }
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
            }
            file.flush().await.map_err(|e| e.to_string())
        }
        .await;
        if let Err(error) = download {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error);
        }
        downloaded = true;
        path
    };
    let result = async {
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(|e| e.to_string())?
            .len();
        let sniff_path = path.clone();
        let kind = tokio::task::spawn_blocking(move || {
            Ok::<_, String>(
                infer::get_from_path(sniff_path)
                    .map_err(|e| e.to_string())?
                    .map(|kind| kind.mime_type().to_string()),
            )
        })
        .await
        .map_err(|e| e.to_string())??;
        let mime = match kind {
            Some(mime) => mime,
            None if size <= STORE_FILE_IPC_MAX_BYTES as u64 => {
                let input = tokio::fs::File::open(&path)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                input
                    .take(STORE_FILE_IPC_MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                if bytes.len() > STORE_FILE_IPC_MAX_BYTES {
                    return Err("SVG 图片超过 8 MiB".into());
                }
                let markup = String::from_utf8(bytes).map_err(|_| "无法识别图片文件格式")?;
                if !is_svg_candidate(&markup) {
                    return Err("无法识别图片文件格式".into());
                }
                return Ok(RenderedImage {
                    path: path.to_string_lossy().into_owned(),
                    display_path: String::new(),
                    mime: "image/svg+xml".into(),
                    size,
                    width: 0,
                    height: 0,
                    display_mime: String::new(),
                    display_size: 0,
                    svg_markup: Some(markup),
                });
            }
            None => return Err("无法识别图片格式或 SVG 超过 8 MiB".into()),
        };
        if !mime.starts_with("image/") {
            return Err("文件不是可识别图片".into());
        }
        let image = display_image(&app_handle, path.clone(), None).await?;
        Ok(RenderedImage {
            path: path.to_string_lossy().into_owned(),
            display_path: image.path,
            mime,
            size,
            width: image.width,
            height: image.height,
            display_mime: image.mime,
            display_size: image.size,
            svg_markup: None,
        })
    }
    .await;
    match result {
        Ok(image) => Ok(image),
        Err(error) => {
            if downloaded {
                let _ = tokio::fs::remove_file(path).await;
            }
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn release_rendered_image(
    app_handle: AppHandle,
    path: String,
    display_path: String,
) -> Result<(), String> {
    if !display_path.is_empty() {
        release_attachment_image(app_handle.clone(), display_path).await?;
    }
    let root = rendered_root(&app_handle)?;
    // A source CAS or caller-owned export has a different owner and must remain intact.
    if Path::new(&path).parent() == Some(root.as_path()) {
        remove_owned_file(root, path).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avif_display_borrows_a_real_encoded_image_without_owning_its_file() {
        // Generated with Pillow/libavif: one static 32x16 RGB image.
        let path = PathBuf::from("tests/fixtures/images/display.avif");
        let size = std::fs::metadata(&path).unwrap().len();
        let file = AttachmentCasFile {
            path: path.clone(),
            mime_type: "image/avif".into(),
            size_bytes: size,
            sha256: "unused-by-display".into(),
        };
        let image = borrow_avif_display(&file).unwrap();
        assert!(!image.owned);
        assert_eq!(image.image.path, path.to_string_lossy());
        assert_eq!((image.image.width, image.image.height), (32, 16));
        assert_eq!(image.image.size, size);
        assert_eq!(image.image.mime, "image/avif");
        drop(image);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

        let changed = AttachmentCasFile { size_bytes: size + 1, ..file };
        assert!(borrow_avif_display(&changed).is_err());
    }

    #[test]
    fn truncated_text_does_not_invent_a_replacement_character() {
        assert_eq!(
            decode_preview_with_encoding(&[b'a', 0xe4, 0xb8], false).0,
            "a"
        );
        assert_eq!(
            decode_preview_with_encoding("中文".as_bytes(), true).0,
            "中文"
        );
        assert_eq!(
            decode_preview_with_encoding(&[0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65], true).0,
            "中文"
        );
    }
    #[test]
    fn asset_urls_resolve_encoded_paths_without_becoming_network_requests() {
        assert_eq!(
            local_image_path("asset://localhost/%2Fvar%2Fimage%20name.png").expect("url"),
            Some(PathBuf::from("//var/image name.png"))
        );
        assert!(local_image_path("https://example.com/image.png")
            .expect("remote")
            .is_none());
    }
}
