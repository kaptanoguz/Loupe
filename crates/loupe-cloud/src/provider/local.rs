//! Yerel disk ya da bağlanmış NAS (NFS/SMB) hedefi.
//!
//! Yazma: aynı dizinde gizli geçici dosya → fsync → `rename` → dizin fsync.
//! Yarıda kalan yükleme hedef adı hiç oluşturmaz. `head_object` SHA-256'yı
//! dosyadan okuyarak hesaplar: yükleme sırasında akıştan hesaplananla
//! karşılaştırılır.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::{ByteStream, ChecksumAlgo, CloudProvider, ObjectMeta, RemoteChecksum, UploadReceipt, check_key, hex};
use crate::CloudError;

#[derive(Debug, Clone)]
pub struct LocalProvider {
    root: PathBuf,
}

impl LocalProvider {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, CloudError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_absolute() {
            return Err(CloudError::Config(format!("yerel hedef mutlak yol olmalı: {}", root.display())));
        }
        Ok(Self { root })
    }

    fn path(&self, key: &str) -> Result<PathBuf, CloudError> {
        check_key(key)?;
        Ok(self.root.join(key))
    }
}

fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ignored = d.sync_all();
    }
}

#[async_trait]
impl CloudProvider for LocalProvider {
    fn name(&self) -> &str {
        "local"
    }

    async fn upload_stream(&self, key: &str, mut body: ByteStream) -> Result<UploadReceipt, CloudError> {
        let target = self.path(key)?;
        let dir = target.parent().ok_or_else(|| CloudError::Config("anahtarın dizini yok".into()))?.to_path_buf();
        tokio::fs::create_dir_all(&dir).await.map_err(CloudError::io("hedef dizin"))?;
        let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("nesne");
        let tmp = dir.join(format!(".{name}.{}.part", rand::random::<u64>()));
        let mut file = tokio::fs::File::create(&tmp).await.map_err(CloudError::io("geçici nesne"))?;

        let mut sha = Sha256::new();
        let mut size = 0u64;
        let written: Result<(), CloudError> = async {
            while let Some(chunk) = body.next().await {
                let chunk = chunk?;
                sha.update(&chunk);
                size += chunk.len() as u64;
                file.write_all(&chunk).await.map_err(CloudError::io("nesne yazma"))?;
            }
            file.flush().await.map_err(CloudError::io("nesne yazma"))?;
            file.sync_all().await.map_err(CloudError::io("nesne fsync"))?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = written {
            let _ignored = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        tokio::fs::rename(&tmp, &target).await.map_err(CloudError::io("nesne yerleştirme"))?;
        tokio::task::spawn_blocking(move || sync_dir(&dir)).await?;
        Ok(UploadReceipt {
            size,
            expected: Some(RemoteChecksum { algo: ChecksumAlgo::Sha256, value: hex(&sha.finalize()) }),
        })
    }

    async fn download_stream(&self, key: &str) -> Result<ByteStream, CloudError> {
        let path = self.path(key)?;
        let file = tokio::fs::File::open(&path).await.map_err(CloudError::io("nesne okuma"))?;
        let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024)
            .map_err(|source| CloudError::Io { context: "nesne okuma", source });
        Ok(Box::pin(stream))
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, CloudError> {
        let path = self.path(key)?;
        tokio::task::spawn_blocking(move || {
            let mut f = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(CloudError::Io { context: "nesne üst verisi", source: e }),
            };
            let mut sha = Sha256::new();
            let size = std::io::copy(&mut f, &mut sha).map_err(CloudError::io("nesne okuma"))?;
            Ok(Some(ObjectMeta {
                size,
                checksum: Some(RemoteChecksum { algo: ChecksumAlgo::Sha256, value: hex(&sha.finalize()) }),
            }))
        })
        .await?
    }

    async fn delete_object(&self, key: &str) -> Result<(), CloudError> {
        let path = self.path(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CloudError::Io { context: "nesne silme", source: e }),
        }
    }
}
