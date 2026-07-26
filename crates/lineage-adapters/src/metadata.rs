use lineage_core::Conversation;
use serde_json::Value;

pub fn is_real_model(model: &str) -> bool {
    !model.is_empty() && model != "<synthetic>"
}

pub fn normalize_model(model: Option<String>) -> Option<String> {
    model.filter(|m| is_real_model(m))
}

pub fn insert_str(
    meta: &mut std::collections::HashMap<String, Value>,
    key: &str,
    value: Option<String>,
) {
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        meta.insert(key.into(), Value::String(v));
    }
}

pub fn finalize_session_metadata(conversation: &mut Conversation) {
    conversation.sync_models_metadata();
}

/// The vendor id an adapter wrote at import, under the key that adapter owns.
/// Each caller passes its own key: the mapping from agent to metadata key is
/// adapter knowledge, so there is deliberately no lookup table here.
pub fn vendor_session_id<'a>(conversation: &'a Conversation, key: &str) -> Option<&'a str> {
    conversation
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}
