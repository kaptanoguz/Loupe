//! Kaynak denetimi: duraklat/devam, kaynak modu (eşzamanlılık) ve kullanıcı
//! etkinliği. Görünen medyanın işleri (`Immediate`) hiçbir kısıtta durmaz;
//! aksi halde kaydırılan ızgara boş kalırdı.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::model::Priority;

/// Kullanıcının seçtiği kaynak modu: kaç genel işçi aynı anda çalışabilir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceMode {
    /// Tek genel işçi (pilde, oyun oynarken).
    Quiet,
    /// Genel işçilerin yarısı (varsayılan).
    Balanced,
    /// Tüm işçiler (ilk içe aktarma, makine boşta).
    Full,
}

impl ResourceMode {
    const fn to_u8(self) -> u8 {
        match self {
            Self::Quiet => 0,
            Self::Balanced => 1,
            Self::Full => 2,
        }
    }

    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Quiet,
            1 => Self::Balanced,
            _ => Self::Full,
        }
    }
}

#[derive(Debug)]
pub struct Throttle {
    paused: AtomicBool,
    mode: AtomicU8,
    user_active: AtomicBool,
    importing: AtomicBool,
    general_workers: usize,
}

impl Throttle {
    pub(crate) const fn new(general_workers: usize, mode: ResourceMode) -> Self {
        Self {
            paused: AtomicBool::new(false),
            mode: AtomicU8::new(mode.to_u8()),
            user_active: AtomicBool::new(false),
            importing: AtomicBool::new(false),
            general_workers,
        }
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub(crate) fn set_mode(&self, mode: ResourceMode) {
        self.mode.store(mode.to_u8(), Ordering::SeqCst);
    }

    #[must_use]
    pub fn mode(&self) -> ResourceMode {
        ResourceMode::from_u8(self.mode.load(Ordering::SeqCst))
    }

    pub(crate) fn set_user_active(&self, active: bool) {
        self.user_active.store(active, Ordering::SeqCst);
    }

    pub(crate) fn set_importing(&self, importing: bool) {
        self.importing.store(importing, Ordering::SeqCst);
    }

    /// Moda göre aynı anda çalışabilecek genel işçi sayısı (en az 1).
    #[must_use]
    pub fn general_limit(&self) -> usize {
        let n = self.general_workers.max(1);
        match self.mode() {
            ResourceMode::Quiet => 1,
            ResourceMode::Balanced => n.div_ceil(2),
            ResourceMode::Full => n,
        }
    }

    /// `general_index` numaralı genel işçinin şu an alabileceği en düşük
    /// öncelik seviyesi (dahil). Ayrılmış işçiler için `None` verilir.
    #[must_use]
    pub fn max_priority(&self, general_index: Option<usize>) -> u8 {
        let immediate = Priority::Immediate.level();
        let Some(g) = general_index else { return immediate };
        if self.is_paused() || g >= self.general_limit() {
            return immediate;
        }
        if self.user_active.load(Ordering::SeqCst) || self.importing.load(Ordering::SeqCst) {
            // Kullanıcı etkinken ya da içe aktarma sürerken toplu işler (Normal
            // ve altı) bekler; diski ve işlemciyi aktarma kullanır.
            return Priority::Normal.level() - 1;
        }
        Priority::Idle.level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_by_mode_and_activity() {
        let t = Throttle::new(4, ResourceMode::Balanced);
        assert_eq!(t.general_limit(), 2);
        assert_eq!(t.max_priority(Some(0)), 9);
        assert_eq!(t.max_priority(Some(2)), 0, "limit dışındaki işçi yalnızca acil iş alır");
        assert_eq!(t.max_priority(None), 0);
        t.set_importing(true);
        assert_eq!(t.max_priority(Some(0)), 4, "içe aktarma sürerken yalnızca ızgara küçük resmi");
        t.set_importing(false);
        assert_eq!(t.max_priority(Some(0)), 9);
        t.set_user_active(true);
        assert_eq!(t.max_priority(Some(0)), 4);
        t.set_paused(true);
        assert_eq!(t.max_priority(Some(0)), 0);
        t.set_mode(ResourceMode::Quiet);
        assert_eq!(t.general_limit(), 1);
        t.set_mode(ResourceMode::Full);
        assert_eq!(t.general_limit(), 4);
    }
}
