use serde::{Deserialize, Deserializer, Serialize};

fn string_or_number<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        U64(u64),
        I64(i64),
        F64(f64),
    }
    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::String(s) => Ok(s),
        StringOrNumber::U64(n) => Ok(n.to_string()),
        StringOrNumber::I64(n) => Ok(n.to_string()),
        StringOrNumber::F64(n) => Ok(n.to_string()),
    }
}

fn nullable_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteUser {
    pub user_id: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub nickname: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub avatar: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteEnv {
    #[serde(default, deserialize_with = "string_or_number")]
    pub id: String,
    pub user_id: String,
    pub name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub rule: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SyncReason {
    #[default]
    Disabled,
    Reachable,
    Unreachable,
    Unauthorized,
    Ready,
    Syncing,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroup {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub avatar: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub description: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub visibility: String,
    pub level: Option<i32>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroupMember {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub level: i32,
    #[serde(default, deserialize_with = "nullable_string")]
    pub nickname: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub avatar: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub email: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGroupSetting {
    pub group_id: String,
    #[serde(default)]
    pub rules_enabled: bool,
    #[serde(default)]
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGroupReq {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteGroupReq {
    pub user_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupSettingReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupListResponse {
    pub list: Vec<RemoteGroup>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberListResponse {
    pub list: Vec<RemoteGroupMember>,
    pub total: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_or_number_accepts_string_and_numbers() {
        // String passthrough.
        let env: RemoteEnv =
            serde_json::from_value(serde_json::json!({"id":"abc","user_id":"u","name":"n"}))
                .unwrap();
        assert_eq!(env.id, "abc");
        // u64 / i64 / f64 all coerce to their string form.
        let env: RemoteEnv =
            serde_json::from_value(serde_json::json!({"id":42,"user_id":"u","name":"n"})).unwrap();
        assert_eq!(env.id, "42");
        let env: RemoteEnv =
            serde_json::from_value(serde_json::json!({"id":-7,"user_id":"u","name":"n"})).unwrap();
        assert_eq!(env.id, "-7");
        let env: RemoteEnv =
            serde_json::from_value(serde_json::json!({"id":1.5,"user_id":"u","name":"n"})).unwrap();
        assert_eq!(env.id, "1.5");
    }

    #[test]
    fn nullable_string_maps_null_to_empty() {
        let user: RemoteUser = serde_json::from_value(serde_json::json!({
            "user_id": "u",
            "nickname": null,
            "avatar": null,
        }))
        .unwrap();
        assert_eq!(user.nickname, "");
        assert_eq!(user.avatar, "");
        assert_eq!(user.email, ""); // missing -> default
    }

    #[test]
    fn nullable_string_keeps_present_value() {
        let user: RemoteUser = serde_json::from_value(serde_json::json!({
            "user_id": "u",
            "nickname": "Alice",
        }))
        .unwrap();
        assert_eq!(user.nickname, "Alice");
    }

    #[test]
    fn sync_reason_default_and_serde_round_trip() {
        assert_eq!(SyncReason::default(), SyncReason::Disabled);
        let json = serde_json::to_string(&SyncReason::Unauthorized).unwrap();
        assert_eq!(json, "\"unauthorized\"");
        let back: SyncReason = serde_json::from_str("\"syncing\"").unwrap();
        assert_eq!(back, SyncReason::Syncing);
    }

    #[test]
    fn remote_group_parses_with_optional_fields() {
        let g: RemoteGroup = serde_json::from_value(serde_json::json!({
            "id": "g1",
            "name": "Group",
            "level": 3,
            "created_by": "u1",
        }))
        .unwrap();
        assert_eq!(g.id, "g1");
        assert_eq!(g.level, Some(3));
        assert_eq!(g.created_by.as_deref(), Some("u1"));
        assert_eq!(g.avatar, "");
    }

    #[test]
    fn request_payloads_skip_none_fields() {
        let req = UpdateGroupReq {
            name: Some("new".into()),
            avatar: None,
            description: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json, serde_json::json!({"name":"new"}));

        let setting = UpdateGroupSettingReq {
            rules_enabled: Some(true),
            visibility: None,
        };
        assert_eq!(
            serde_json::to_value(&setting).unwrap(),
            serde_json::json!({"rules_enabled": true})
        );
    }

    #[test]
    fn list_responses_round_trip() {
        let resp = GroupListResponse {
            list: vec![],
            total: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: GroupListResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total, 0);

        let member = GroupMemberListResponse {
            list: vec![],
            total: 5,
        };
        let back: GroupMemberListResponse =
            serde_json::from_str(&serde_json::to_string(&member).unwrap()).unwrap();
        assert_eq!(back.total, 5);
    }

    #[test]
    fn create_and_invite_requests_serialize() {
        let create = CreateGroupReq {
            name: "g".into(),
            avatar: None,
            description: Some("d".into()),
            visibility: None,
        };
        assert_eq!(
            serde_json::to_value(&create).unwrap(),
            serde_json::json!({"name":"g","description":"d"})
        );
        let invite = InviteGroupReq {
            user_ids: vec!["a".into(), "b".into()],
            level: Some(2),
        };
        let json = serde_json::to_value(&invite).unwrap();
        assert_eq!(json["user_ids"], serde_json::json!(["a", "b"]));
        assert_eq!(json["level"], 2);
    }

    #[test]
    fn remote_group_member_and_setting_parse() {
        let m: RemoteGroupMember = serde_json::from_value(serde_json::json!({
            "id":"m1","group_id":"g1","user_id":"u1","level":1
        }))
        .unwrap();
        assert_eq!(m.level, 1);
        assert_eq!(m.email, "");

        let s: RemoteGroupSetting = serde_json::from_value(serde_json::json!({
            "group_id":"g1"
        }))
        .unwrap();
        assert!(!s.rules_enabled);
        assert_eq!(s.visibility, "");
    }
}
