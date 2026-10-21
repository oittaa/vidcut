use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Lru<K, V, const N: usize> {
    entries: VecDeque<(K, V)>,
}

impl<K, V, const N: usize> Default for Lru<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, const N: usize> Lru<K, V, N> {
    pub const fn new() -> Self {
        const { assert!(N > 0) };
        Self {
            entries: VecDeque::new(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<K: PartialEq, V, const N: usize> Lru<K, V, N> {
    fn position(&self, key: &K) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k == key)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.position(key).is_some()
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        let hit = self.entries.remove(self.position(key)?)?;
        self.entries.push_back(hit);
        self.entries.back().map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(i) = self.position(&key) {
            self.entries.remove(i);
        } else if self.entries.len() == N {
            self.entries.pop_front();
        }
        self.entries.push_back((key, value));
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let (_, v) = self.entries.remove(self.position(key)?)?;
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::Lru;

    fn keys<const N: usize>(lru: &Lru<u32, &str, N>) -> Vec<u32> {
        lru.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn a_kept_value_is_got_back() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        assert_eq!(lru.get(&1), Some(&"a"));
    }

    #[test]
    fn past_its_size_the_least_recently_used_goes() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        lru.insert(2, "b");
        lru.insert(3, "c");
        assert_eq!(keys(&lru), [2, 3]);
    }

    #[test]
    fn a_get_marks_the_key_as_used() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        lru.insert(2, "b");
        lru.get(&1);
        lru.insert(3, "c");
        assert_eq!(keys(&lru), [1, 3]);
    }

    #[test]
    fn contains_leaves_the_order_as_it_is() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        lru.insert(2, "b");
        assert!(lru.contains(&1));
        lru.insert(3, "c");
        assert_eq!(keys(&lru), [2, 3]);
    }

    #[test]
    fn inserting_a_kept_key_replaces_its_value() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        lru.insert(2, "b");
        lru.insert(1, "c");
        assert_eq!(lru.iter().collect::<Vec<_>>(), [(&2, &"b"), (&1, &"c")]);
    }

    #[test]
    fn a_removed_key_gives_its_value_back() {
        let mut lru: Lru<u32, &str, 2> = Lru::new();
        lru.insert(1, "a");
        lru.insert(2, "b");
        assert_eq!(lru.remove(&1), Some("a"));
        assert_eq!(keys(&lru), [2]);
    }
}
