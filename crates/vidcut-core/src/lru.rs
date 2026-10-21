use std::collections::VecDeque;

#[derive(Debug)]
pub struct Lru<K, V, const N: usize>(VecDeque<(K, V)>);

impl<K, V, const N: usize> Default for Lru<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, const N: usize> Lru<K, V, N> {
    pub const fn new() -> Self {
        Self(VecDeque::new())
    }

    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.0.iter()
    }
}

impl<K: PartialEq, V, const N: usize> Lru<K, V, N> {
    pub fn contains(&self, key: &K) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    pub fn remove(&mut self, key: &K) -> Option<(K, V)> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        self.0.remove(i)
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        let hit = self.remove(key)?;
        self.0.push_back(hit);
        self.0.back().map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.remove(&key).is_none() && self.0.len() == N {
            self.0.pop_front();
        }
        self.0.push_back((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::Lru;

    #[test]
    fn past_its_size_the_least_recently_used_goes() {
        let mut lru: Lru<u32, (), 2> = Lru::new();
        lru.insert(1, ());
        lru.insert(2, ());
        lru.get(&1);
        lru.insert(3, ());
        assert_eq!(lru.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [1, 3]);
    }
}
