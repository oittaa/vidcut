use std::borrow::Borrow;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    pub fn into_inner(self) -> T
    where
        T: Clone,
    {
        Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone())
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: fmt::Debug> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl<T: Eq> Eq for Shared<T> {}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T> AsRef<T> for Shared<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> AsMut<T> for Shared<T> {
    fn as_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: Serialize> Serialize for Shared<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Shared<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::new)
    }
}

impl<T: JsonSchema> JsonSchema for Shared<T> {
    fn inline_schema() -> bool {
        T::inline_schema()
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        T::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

pub struct SharedVec<T>(Arc<Vec<T>>);

impl<T> SharedVec<T> {
    pub fn new() -> Self {
        Self(Arc::new(Vec::new()))
    }

    pub fn from_vec(v: Vec<T>) -> Self {
        Self(Arc::new(v))
    }

    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<T>
    where
        T: Clone,
    {
        Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone())
    }
}

impl<T> Clone for SharedVec<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for SharedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for SharedVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: PartialEq> PartialEq for SharedVec<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl<T: PartialEq> PartialEq<Vec<T>> for SharedVec<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        *self.0 == *other
    }
}

impl<T: PartialEq> PartialEq<[T]> for SharedVec<T> {
    fn eq(&self, other: &[T]) -> bool {
        self.0.as_slice() == other
    }
}

impl<T: PartialEq> PartialEq<&[T]> for SharedVec<T> {
    fn eq(&self, other: &&[T]) -> bool {
        self.0.as_slice() == *other
    }
}

impl<T: PartialEq, const N: usize> PartialEq<[T; N]> for SharedVec<T> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.0.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for SharedVec<T> {}

impl<T> From<Vec<T>> for SharedVec<T> {
    fn from(v: Vec<T>) -> Self {
        Self(Arc::new(v))
    }
}

impl<T: Clone> From<&[T]> for SharedVec<T> {
    fn from(s: &[T]) -> Self {
        Self(Arc::new(s.to_vec()))
    }
}

impl<T, const N: usize> From<[T; N]> for SharedVec<T> {
    fn from(a: [T; N]) -> Self {
        Self(Arc::new(a.into()))
    }
}

impl<T> FromIterator<T> for SharedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(Arc::new(iter.into_iter().collect()))
    }
}

impl<T: Clone> Extend<T> for SharedVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        Arc::make_mut(&mut self.0).extend(iter);
    }
}

impl<T> Deref for SharedVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T: Clone> DerefMut for SharedVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        Arc::make_mut(&mut self.0)
    }
}

impl<T> AsRef<[T]> for SharedVec<T> {
    fn as_ref(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<T> AsRef<Vec<T>> for SharedVec<T> {
    fn as_ref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> Borrow<[T]> for SharedVec<T> {
    fn borrow(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<'a, T> IntoIterator for &'a SharedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut SharedVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Arc::make_mut(&mut self.0).iter_mut()
    }
}

impl<T: Clone> IntoIterator for SharedVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<T: Serialize> Serialize for SharedVec<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SharedVec<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(d).map(Self::from)
    }
}

impl<T: JsonSchema> JsonSchema for SharedVec<T> {
    fn inline_schema() -> bool {
        Vec::<T>::inline_schema()
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        Vec::<T>::schema_name()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        Vec::<T>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        Vec::<T>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloning_a_shared_vec_shares_until_written() {
        let a = SharedVec::from(vec![1, 2, 3]);
        let mut b = a.clone();
        assert!(SharedVec::ptr_eq(&a, &b));
        b.push(4);
        assert!(!SharedVec::ptr_eq(&a, &b));
        assert_eq!(a, [1, 2, 3]);
        assert_eq!(b, [1, 2, 3, 4]);
    }

    #[test]
    fn shared_vec_schema_is_an_array_of_its_items() {
        let schema = schemars::schema_for!(SharedVec<String>);
        let val = schema.as_value();
        assert_eq!(val["type"], serde_json::json!("array"));
        assert_eq!(val["items"], serde_json::json!({"type": "string"}));
    }
}
