//! Serde helpers for wire shapes TypeScript models with both `?` and `| null`.

pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// `None` omits the key (the caller skips it), `Some(None)` writes `null`,
    /// `Some(Some(v))` writes `v`. The `None` arm is only reachable when the
    /// field is serialized without `skip_serializing_if`.
    pub fn serialize<T, S>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        match value {
            Some(Some(inner)) => inner.serialize(serializer),
            Some(None) | None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}
