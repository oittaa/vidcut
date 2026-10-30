use std::collections::HashMap;
use std::hash::{BuildHasherDefault, DefaultHasher, Hash};

#[derive(Debug)]
struct Node<K, V> {
    key: K,
    value: V,
    older: Option<usize>,
    newer: Option<usize>,
}

#[derive(Debug)]
pub struct Lru<K, V, const N: usize> {
    at: HashMap<K, usize, BuildHasherDefault<DefaultHasher>>,
    nodes: Vec<Node<K, V>>,
    oldest: Option<usize>,
    newest: Option<usize>,
}

impl<K, V, const N: usize> Default for Lru<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, const N: usize> Lru<K, V, N> {
    pub const fn new() -> Self {
        Self {
            at: HashMap::with_hasher(BuildHasherDefault::new()),
            nodes: Vec::new(),
            oldest: None,
            newest: None,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        std::iter::successors(self.oldest, |&i| self.nodes[i].newer).map(|i| {
            let n = &self.nodes[i];
            (&n.key, &n.value)
        })
    }

    fn unlink(&mut self, i: usize) {
        let (older, newer) = (self.nodes[i].older, self.nodes[i].newer);
        match older {
            Some(o) => self.nodes[o].newer = newer,
            None => self.oldest = newer,
        }
        match newer {
            Some(n) => self.nodes[n].older = older,
            None => self.newest = older,
        }
    }

    fn link_newest(&mut self, i: usize) {
        self.nodes[i].older = self.newest;
        self.nodes[i].newer = None;
        match self.newest {
            Some(n) => self.nodes[n].newer = Some(i),
            None => self.oldest = Some(i),
        }
        self.newest = Some(i);
    }
}

impl<K: Hash + Eq + Clone, V, const N: usize> Lru<K, V, N> {
    pub fn contains(&self, key: &K) -> bool {
        self.at.contains_key(key)
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.at.get(key).map(|&i| &self.nodes[i].value)
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.get_mut(key).map(|v| &*v)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let i = *self.at.get(key)?;
        if self.newest != Some(i) {
            self.unlink(i);
            self.link_newest(i);
        }
        Some(&mut self.nodes[i].value)
    }

    pub fn pop_oldest(&mut self) -> Option<(K, V)> {
        let k = self.nodes[self.oldest?].key.clone();
        self.remove(&k)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(&i) = self.at.get(&key) {
            self.nodes[i].value = value;
            self.unlink(i);
            self.link_newest(i);
            return;
        }
        if self.nodes.len() >= N {
            self.pop_oldest();
        }
        let i = self.nodes.len();
        self.at.insert(key.clone(), i);
        self.nodes.push(Node {
            key,
            value,
            older: None,
            newer: None,
        });
        self.link_newest(i);
    }

    pub fn clear(&mut self) {
        self.at.clear();
        self.nodes.clear();
        self.oldest = None;
        self.newest = None;
    }

    pub fn remove(&mut self, key: &K) -> Option<(K, V)> {
        let i = self.at.remove(key)?;
        self.unlink(i);
        let last = self.nodes.len() - 1;
        if i != last {
            let (older, newer) = (self.nodes[last].older, self.nodes[last].newer);
            match older {
                Some(o) => self.nodes[o].newer = Some(i),
                None => self.oldest = Some(i),
            }
            match newer {
                Some(n) => self.nodes[n].older = Some(i),
                None => self.newest = Some(i),
            }
            if let Some(at) = self.at.get_mut(&self.nodes[last].key) {
                *at = i;
            }
        }
        let gone = self.nodes.swap_remove(i);
        Some((gone.key, gone.value))
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

    #[test]
    fn a_cleared_cache_finds_nothing() {
        let mut lru: Lru<u32, (), 2> = Lru::new();
        lru.insert(1, ());
        lru.clear();
        assert_eq!(lru.get(&1), None);
    }
}
