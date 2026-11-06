//! Owner and group names of the long view, resolved once per distinct id.

/// Names by id, each table sorted by id; `None` for an id without a name.
#[derive(Default)]
pub struct Owners {
    users: Vec<(u32, Option<String>)>,
    groups: Vec<(u32, Option<String>)>,
}

impl Owners {
    /// Resolves the names of the owners and groups of `pairs`, `(uid, gid)`.
    pub fn resolve(pairs: &[(u32, u32)]) -> Self {
        fn names(mut ids: Vec<u32>, get: fn(u32) -> Option<String>) -> Vec<(u32, Option<String>)> {
            ids.sort_unstable();
            ids.dedup();
            ids.into_iter().map(|id| (id, get(id))).collect()
        }

        Self {
            users: names(
                pairs.iter().map(|o| o.0).collect(),
                crate::fs::ug::user_name,
            ),
            groups: names(
                pairs.iter().map(|o| o.1).collect(),
                crate::fs::ug::group_name,
            ),
        }
    }

    /// The names of owner `uid` and group `gid`, if both were resolved.
    pub fn get(&self, uid: u32, gid: u32) -> Option<(&str, &str)> {
        Some((lookup(&self.users, uid)?, lookup(&self.groups, gid)?))
    }
}

/// The name of `id` in `table`, sorted by id.
fn lookup(table: &[(u32, Option<String>)], id: u32) -> Option<&str> {
    let k = table.binary_search_by_key(&id, |(i, _)| *i).ok()?;
    table[k].1.as_deref()
}
