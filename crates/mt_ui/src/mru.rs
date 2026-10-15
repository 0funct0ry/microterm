//! Most-recently-used palette items (SPEC §10.1): the last 50 executed item ids, newest first,
//! persisted to `$XDG_STATE_HOME/microterm/mru.json`.

use std::path::Path;

pub const MRU_CAPACITY: usize = 50;
const MRU_FILE: &str = "mru.json";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mru {
    /// Newest first.
    ids: Vec<String>,
}

impl Mru {
    /// Moves (or inserts) `id` to the front, dropping the oldest beyond the capacity.
    pub fn touch(&mut self, id: &str) {
        self.ids.retain(|i| i != id);
        self.ids.insert(0, id.to_string());
        self.ids.truncate(MRU_CAPACITY);
    }

    /// 0 = most recent; `None` when the item was never run.
    pub fn rank(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|i| i == id)
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// A missing or corrupt file is an empty history.
    pub fn load(dir: &Path) -> Self {
        let ids = std::fs::read_to_string(dir.join(MRU_FILE))
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default();
        let mut mru = Self::default();
        for id in ids.iter().rev() {
            mru.touch(id);
        }
        mru
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{MRU_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string(&self.ids)?)?;
        std::fs::rename(tmp, dir.join(MRU_FILE))
    }
}

/// Process-wide MRU shared by every window. Persists under the state dir; unit tests keep it in
/// memory so they never touch the user's real history.
pub struct MruStore {
    mru: Mru,
    dir: Option<std::path::PathBuf>,
}

impl gpui::Global for MruStore {}

impl MruStore {
    fn ensure(cx: &mut gpui::App) {
        if !cx.has_global::<Self>() {
            let dir = if cfg!(test) {
                None
            } else {
                crate::state::state_dir()
            };
            let mru = dir.as_deref().map(Mru::load).unwrap_or_default();
            cx.set_global(Self { mru, dir });
        }
    }

    /// Rank of every id in `ids`, for ranking the picker.
    pub fn ranks(cx: &mut gpui::App, ids: &[String]) -> Vec<Option<usize>> {
        Self::ensure(cx);
        let store = cx.global::<Self>();
        ids.iter().map(|i| store.mru.rank(i)).collect()
    }

    /// Records an executed item and persists the history (errors are ignored: it is a cache).
    pub fn record(cx: &mut gpui::App, id: &str) {
        Self::ensure(cx);
        let store = cx.global_mut::<Self>();
        store.mru.touch(id);
        if let Some(dir) = &store.dir {
            let _ = store.mru.save(dir);
        }
    }

    pub fn current(cx: &mut gpui::App) -> Mru {
        Self::ensure(cx);
        cx.global::<Self>().mru.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mt-mru-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn touch_orders_newest_first_without_duplicates() {
        let mut m = Mru::default();
        m.touch("a");
        m.touch("b");
        m.touch("a");
        assert_eq!(m.ids(), ["a", "b"]);
        assert_eq!(m.rank("a"), Some(0));
        assert_eq!(m.rank("b"), Some(1));
        assert_eq!(m.rank("c"), None);
    }

    #[test]
    fn capped_at_fifty() {
        let mut m = Mru::default();
        for i in 0..80 {
            m.touch(&format!("id{i}"));
        }
        assert_eq!(m.ids().len(), MRU_CAPACITY);
        assert_eq!(m.rank("id79"), Some(0));
        assert_eq!(m.rank("id0"), None);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp("roundtrip");
        let mut m = Mru::default();
        m.touch("x");
        m.touch("y");
        m.save(&dir).unwrap();
        assert_eq!(Mru::load(&dir), m);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_or_missing_file_is_empty() {
        let dir = temp("corrupt");
        assert_eq!(Mru::load(&dir), Mru::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MRU_FILE), "{not json").unwrap();
        assert_eq!(Mru::load(&dir), Mru::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
