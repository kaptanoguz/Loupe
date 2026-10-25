//! Tablo satır kimlikleri. Karıştırılmasınlar diye her tablo kendi tipini alır.

use std::fmt;

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub i64);

        impl $name {
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

row_id!(
    /// `libraries.id`
    LibraryId
);
row_id!(
    /// `media_files.id`
    MediaId
);
row_id!(
    /// `import_sessions.id`
    ImportSessionId
);
row_id!(
    /// `import_items.id`
    ImportItemId
);
row_id!(
    /// `verify_runs.id`
    VerifyRunId
);
