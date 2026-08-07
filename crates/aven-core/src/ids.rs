use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use getrandom::fill as fill_random;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::database::Database;
use sqlx::decode::Decode;
use sqlx::encode::{Encode, IsNull};
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteTypeInfo, SqliteValueRef};
use sqlx::types::Type;

pub const BASE32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProjectId(String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidWorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTaskId;

#[allow(clippy::new_without_default)]
impl WorkspaceId {
    pub fn new() -> Self {
        Self(new_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[allow(clippy::new_without_default)]
impl ProjectId {
    pub fn new() -> Self {
        Self(new_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[allow(clippy::new_without_default)]
impl TaskId {
    pub fn new() -> Self {
        Self(new_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for TaskId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for TaskId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for InvalidWorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("workspace ID must be 16 Crockford Base32 characters")
    }
}

impl fmt::Display for InvalidProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("project ID must be 16 Crockford Base32 characters")
    }
}

impl fmt::Display for InvalidTaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("task ID must be 16 Crockford Base32 characters")
    }
}

impl std::error::Error for InvalidWorkspaceId {}
impl std::error::Error for InvalidProjectId {}
impl std::error::Error for InvalidTaskId {}

impl FromStr for WorkspaceId {
    type Err = InvalidWorkspaceId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidWorkspaceId)
        }
    }
}

impl FromStr for ProjectId {
    type Err = InvalidProjectId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidProjectId)
        }
    }
}

impl FromStr for TaskId {
    type Err = InvalidTaskId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidTaskId)
        }
    }
}

impl TryFrom<String> for WorkspaceId {
    type Error = InvalidWorkspaceId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for ProjectId {
    type Error = InvalidProjectId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for TaskId {
    type Error = InvalidTaskId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Type<Sqlite> for WorkspaceId {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl Type<Sqlite> for ProjectId {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl Type<Sqlite> for TaskId {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl Encode<'_, Sqlite> for WorkspaceId {
    fn encode_by_ref(
        &self,
        buffer: &mut <Sqlite as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<Sqlite>>::encode_by_ref(&self.0, buffer)
    }
}

impl Encode<'_, Sqlite> for ProjectId {
    fn encode_by_ref(
        &self,
        buffer: &mut <Sqlite as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<Sqlite>>::encode_by_ref(&self.0, buffer)
    }
}

impl Encode<'_, Sqlite> for TaskId {
    fn encode_by_ref(
        &self,
        buffer: &mut <Sqlite as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<Sqlite>>::encode_by_ref(&self.0, buffer)
    }
}

impl<'row> Decode<'row, Sqlite> for WorkspaceId {
    fn decode(value: SqliteValueRef<'row>) -> Result<Self, BoxDynError> {
        String::decode(value)?.parse().map_err(Into::into)
    }
}

impl<'row> Decode<'row, Sqlite> for ProjectId {
    fn decode(value: SqliteValueRef<'row>) -> Result<Self, BoxDynError> {
        String::decode(value)?.parse().map_err(Into::into)
    }
}

impl<'row> Decode<'row, Sqlite> for TaskId {
    fn decode(value: SqliteValueRef<'row>) -> Result<Self, BoxDynError> {
        String::decode(value)?.parse().map_err(Into::into)
    }
}

pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
        .with_nanosecond(0)
        .expect("UTC timestamps support second precision")
}

pub fn now() -> String {
    now_utc().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn new_id() -> String {
    let mut bytes = [0u8; 10];
    fill_random(&mut bytes).expect("fill random bytes");
    encode_crockford(&bytes)
}

pub fn encode_crockford(bytes: &[u8; 10]) -> String {
    let mut value = u128::from_be_bytes([
        0, 0, 0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
        bytes[7], bytes[8], bytes[9],
    ]);
    let mut chars = [b'0'; 16];
    for i in (0..16).rev() {
        chars[i] = BASE32[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("base32 is utf8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_conn;

    #[test]
    fn timestamps_are_current_utc_values_at_second_precision() {
        let before = Utc::now() - chrono::Duration::seconds(1);
        let typed = now_utc();
        let timestamp = now();
        let after = Utc::now() + chrono::Duration::seconds(1);
        let parsed = DateTime::parse_from_rfc3339(&timestamp).unwrap().to_utc();

        assert_eq!(typed.nanosecond(), 0);
        assert!(typed >= before && typed <= after);
        assert_eq!(timestamp.len(), 20);
        assert_eq!(timestamp, parsed.to_rfc3339_opts(SecondsFormat::Secs, true));
        assert_eq!(parsed.nanosecond(), 0);
        assert!(parsed >= before && parsed <= after);
    }

    #[test]
    fn workspace_ids_validate_domain_id_shape() {
        assert!("0123456789ABCDEF".parse::<WorkspaceId>().is_ok());
        assert!("0123456789ABCDE".parse::<WorkspaceId>().is_err());
        assert!("0123456789abcdef".parse::<WorkspaceId>().is_err());
        assert!("0123456789ABCDEI".parse::<WorkspaceId>().is_err());
    }

    #[test]
    fn workspace_ids_serialize_as_validated_strings() {
        let id: WorkspaceId = serde_json::from_str("\"0123456789ABCDEF\"").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"0123456789ABCDEF\"");
        assert!(serde_json::from_str::<WorkspaceId>("\"invalid\"").is_err());
    }

    #[test]
    fn project_ids_validate_domain_id_shape() {
        assert!("0123456789ABCDEF".parse::<ProjectId>().is_ok());
        assert!("0123456789ABCDE".parse::<ProjectId>().is_err());
        assert!("0123456789abcdef".parse::<ProjectId>().is_err());
        assert!("0123456789ABCDEI".parse::<ProjectId>().is_err());
    }

    #[test]
    fn project_ids_serialize_as_validated_strings() {
        let id: ProjectId = serde_json::from_str("\"0123456789ABCDEF\"").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"0123456789ABCDEF\"");
        assert!(serde_json::from_str::<ProjectId>("\"invalid\"").is_err());
    }

    #[test]
    fn task_ids_validate_domain_id_shape() {
        assert!("0123456789ABCDEF".parse::<TaskId>().is_ok());
        assert!("0123456789ABCDE".parse::<TaskId>().is_err());
        assert!("0123456789abcdef".parse::<TaskId>().is_err());
        assert!("0123456789ABCDEI".parse::<TaskId>().is_err());
    }

    #[test]
    fn task_ids_serialize_as_validated_strings() {
        let id: TaskId = serde_json::from_str("\"0123456789ABCDEF\"").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"0123456789ABCDEF\"");
        assert!(serde_json::from_str::<TaskId>("\"invalid\"").is_err());
    }

    #[tokio::test]
    async fn task_ids_bind_and_decode_as_sqlite_text() {
        let (_temp, mut conn) = test_conn().await;
        let id: TaskId = "0123456789ABCDEF".parse().unwrap();
        let decoded = sqlx::query_scalar::<_, TaskId>("SELECT ?")
            .bind(&id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(decoded, id);

        let invalid = sqlx::query_scalar::<_, TaskId>("SELECT 'invalid'")
            .fetch_one(&mut *conn)
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn project_ids_bind_and_decode_as_sqlite_text() {
        let (_temp, mut conn) = test_conn().await;
        let id: ProjectId = "0123456789ABCDEF".parse().unwrap();
        let decoded = sqlx::query_scalar::<_, ProjectId>("SELECT ?")
            .bind(&id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(decoded, id);

        let invalid = sqlx::query_scalar::<_, ProjectId>("SELECT 'invalid'")
            .fetch_one(&mut *conn)
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn workspace_ids_bind_and_decode_as_sqlite_text() {
        let (_temp, mut conn) = test_conn().await;
        let id: WorkspaceId = "0123456789ABCDEF".parse().unwrap();
        let decoded = sqlx::query_scalar::<_, WorkspaceId>("SELECT ?")
            .bind(&id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(decoded, id);

        let invalid = sqlx::query_scalar::<_, WorkspaceId>("SELECT 'invalid'")
            .fetch_one(&mut *conn)
            .await;
        assert!(invalid.is_err());
    }
}
