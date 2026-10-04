//! Compact framing belongs only to the batch transport; record bytes are unchanged.
use super::*;

pub mod fixed {
    use serde::{Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(
        value: &[u8; 32],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        crate::base64_bytes::serialize(value, serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<[u8; 32], D::Error> {
        crate::base64_bytes::bounded::<D, 32>(deserializer)?
            .try_into()
            .map_err(|_| D::Error::custom("expected 32 bytes"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Context", deny_unknown_fields)]
struct CompactContext {
    #[serde(with = "fixed")]
    vault: [u8; 32],
    #[serde(with = "fixed")]
    genesis: [u8; 32],
    #[serde(with = "fixed")]
    device: [u8; 32],
    #[serde(with = "fixed")]
    head: [u8; 32],
    #[serde(with = "fixed")]
    stream: [u8; 32],
    #[serde(with = "fixed")]
    descriptor: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    #[serde(with = "CompactContext")]
    pub context: Context,
    #[serde(with = "fixed")]
    pub correlation: [u8; 32],
    pub operation: T,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactMapping {
    pub operation_id: String,
    pub sequence: i64,
    #[serde(with = "fixed")]
    pub commitment: [u8; 32],
}
impl From<Mapping> for CompactMapping {
    fn from(value: Mapping) -> Self {
        Self {
            operation_id: value.operation_id,
            sequence: value.sequence,
            commitment: value.commitment,
        }
    }
}
impl From<CompactMapping> for Mapping {
    fn from(value: CompactMapping) -> Self {
        Self {
            operation_id: value.operation_id,
            sequence: value.sequence,
            commitment: value.commitment,
        }
    }
}

const _: () = assert!(BATCH_APPEND_LIMIT <= super::super::images::HTTP_LIMIT);

/// Bounds collection allocation even when an input contains many tiny elements.
pub fn bounded_items<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct Items<T>(std::marker::PhantomData<T>);
    impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most 128 batch items")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            use serde::de::Error;
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == BATCH_COUNT {
                    return Err(A::Error::custom("batch count limit"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    deserializer.deserialize_seq(Items(std::marker::PhantomData))
}
