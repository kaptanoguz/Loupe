//! Kütüphane dosya izleyicisi (inotify; `notify` + `notify-debouncer-full`).
//!
//! Debounce, bir kamera kartından kopyalanan dosyanın onlarca `MODIFY` olayını
//! tek olaya indirger ve `MOVED_FROM`/`MOVED_TO` çiftlerini tek `Renamed`
//! olayına birleştirir. Olaylar bir kanala gönderilir; tüketici (ingest) onları
//! iş kuyruğuna çevirir. Uygulamanın kendi dizinleri (`.loupe`, `.loupe-tmp`) ve
//! importer'ın kendi yazmaları ([`ExpectedPaths`]) olay üretmez.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use notify_debouncer_full::notify::event::{EventKind, ModifyKind, RenameMode};
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer};

use crate::import::ExpectedPaths;
use crate::{LOUPE_DIR, StorageError, TMP_DIR};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Yeni dosya (veya kütüphane dışından içeri taşınan dosya).
    Created(PathBuf),
    /// Yeni dizin: içindekiler olay üretmeden gelmiş olabilir, taranmalı.
    DirCreated(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// Çekirdek olay kuyruğu taştı; kütüphane yeniden taranmalı.
    Rescan,
    Error(String),
}

pub struct LibraryWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    root: PathBuf,
}

impl std::fmt::Debug for LibraryWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryWatcher").field("root", &self.root).finish_non_exhaustive()
    }
}

impl LibraryWatcher {
    pub fn start(
        root: &Path,
        debounce: Duration,
        tx: Sender<WatchEvent>,
        expected: Option<Arc<ExpectedPaths>>,
    ) -> Result<Self, StorageError> {
        let root = crate::fsutil::canonicalize(root).map_err(StorageError::io("izleme kökü", root))?;
        let filter_root = root.clone();
        let handler = move |res: DebounceEventResult| match res {
            Ok(events) => {
                for ev in events {
                    for out in map_event(&filter_root, &ev, expected.as_deref()) {
                        // Alıcı kapandıysa izleyici de kapanmak üzeredir.
                        let _ignored = tx.send(out);
                    }
                }
            }
            Err(errors) => {
                for e in errors {
                    let _ignored = tx.send(WatchEvent::Error(e.to_string()));
                }
            }
        };
        let mut debouncer = new_debouncer(debounce, None, handler)?;
        debouncer.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self { debouncer, root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// İzlemeyi durdurur ve debounce iş parçacığını bekler.
    pub fn stop(self) {
        self.debouncer.stop();
    }
}

fn is_internal(root: &Path, p: &Path) -> bool {
    p.strip_prefix(root)
        .ok()
        .and_then(|rel| rel.components().next())
        .is_some_and(|c| c.as_os_str() == LOUPE_DIR || c.as_os_str() == TMP_DIR)
}

fn map_event(root: &Path, ev: &DebouncedEvent, expected: Option<&ExpectedPaths>) -> Vec<WatchEvent> {
    if ev.need_rescan() {
        return vec![WatchEvent::Rescan];
    }
    let paths = &ev.event.paths;
    let visible = |p: &PathBuf| !is_internal(root, p);
    let ours = |p: &Path| expected.is_some_and(|e| e.take(p));
    let created = |p: &PathBuf| {
        if p.is_dir() { WatchEvent::DirCreated(p.clone()) } else { WatchEvent::Created(p.clone()) }
    };

    match ev.event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if paths.len() == 2 => {
            let (from, to) = (&paths[0], &paths[1]);
            match (visible(from), visible(to)) {
                (true, true) => vec![WatchEvent::Renamed { from: from.clone(), to: to.clone() }],
                // .loupe-tmp → ağaç: importer'ın atomik yerleştirmesi (beklenmiyorsa dış yazma).
                (false, true) if ours(to) => Vec::new(),
                (false, true) => vec![created(to)],
                (true, false) => vec![WatchEvent::Removed(from.clone())],
                (false, false) => Vec::new(),
            }
        }
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            paths.iter().filter(|p| visible(p) && !ours(p)).map(created).collect()
        }
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            paths.iter().filter(|p| visible(p)).map(|p| WatchEvent::Removed(p.clone())).collect()
        }
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any) => paths
            .iter()
            .filter(|p| visible(p) && p.is_file() && !ours(p))
            .map(|p| WatchEvent::Modified(p.clone()))
            .collect(),
        _ => Vec::new(),
    }
}
