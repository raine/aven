//! JSON transport spelling for opaque byte payloads: one canonical padded
//! standard base64 string. The bytes themselves are what callers hash, sign
//! and store.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserializer, Serializer, de};

/// Serialized length of `n` bytes, excluding the JSON quotes.
pub const fn encoded_len(n: usize) -> usize {
    match base64::encoded_len(n, true) {
        Some(len) => len,
        None => panic!("base64 length overflow"),
    }
}

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    deserializer.deserialize_str(Visitor(usize::MAX))
}

/// Refuses a string that would decode to more than `N` bytes before decoding it.
pub fn bounded<'de, D: Deserializer<'de>, const N: usize>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    deserializer.deserialize_str(Visitor(encoded_len(N)))
}

/// The same spelling for an optional payload, with `null` for `None`.
pub mod option {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    struct Bytes(#[serde(with = "super")] Vec<u8>);

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_some(&super::STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(Option::<Bytes>::deserialize(deserializer)?.map(|bytes| bytes.0))
    }
}

struct Visitor(usize);
impl de::Visitor<'_> for Visitor {
    type Value = Vec<u8>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a base64 string of at most {} characters", self.0)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<u8>, E> {
        if value.len() > self.0 {
            return Err(E::custom("byte limit"));
        }
        STANDARD.decode(value).map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Field(#[serde(with = "super")] Vec<u8>);

    #[test]
    fn round_trips_with_exact_length() {
        for n in [0, 1, 2, 3, 4, 255, 1024] {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
            let json = serde_json::to_string(&Field(bytes.clone())).unwrap();
            assert_eq!(json.len(), super::encoded_len(n) + 2);
            assert_eq!(serde_json::from_str::<Field>(&json).unwrap().0, bytes);
        }
    }

    #[test]
    fn rejects_other_spellings() {
        for json in [
            "[1,2,3]",
            "\"AQID\\n\"",
            "\"AQI\"",
            "\"AQJ=\"",
            "\"AQ-_\"",
            "\"AQ==AQ==\"",
        ] {
            assert!(serde_json::from_str::<Field>(json).is_err(), "{json}");
        }
        #[derive(Deserialize)]
        struct Bounded(#[serde(deserialize_with = "super::bounded::<_, 3>")] Vec<u8>);
        assert_eq!(
            serde_json::from_str::<Bounded>("\"AQID\"").unwrap().0,
            [1, 2, 3]
        );
        assert!(serde_json::from_str::<Bounded>("\"AQIDBA==\"").is_err());
        // An escaped JSON string still decodes to the same bytes.
        assert_eq!(
            serde_json::from_str::<Field>("\"\\u0041QID\"").unwrap().0,
            [1, 2, 3]
        );
    }
}
