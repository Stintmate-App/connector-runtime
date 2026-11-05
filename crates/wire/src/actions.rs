//! The two standard actions every connector declares (connector kit, contract.md sections 4.6
//! and 5.1), as data.
//!
//! A connector declares its actions with `pitwall.actions.declare` at every connection
//! (idempotent, `ik` `<prefix>/actions/<sha12 of the actions>`, last declaration wins) and
//! serves them through `connector.action`. This module is the declaration and its key; the
//! handler and the calls are the connector's.

use crate::catalog::{canonical, sha12};
use serde_json::{Value, json};

/// `test.ping`: radio round trip.
pub const TEST_PING: &str = "test.ping";
/// `test.announce`: one announcement of the connector's own catalog.
pub const TEST_ANNOUNCE: &str = "test.announce";
/// How long an action waits for Pitwall to accept its announcement.
pub const ACCEPT_TIMEOUT_S: f64 = 2.0;
/// Timeout of `pitwall.actions.declare`.
pub const DECLARE_TIMEOUT_S: f64 = 5.0;

/// The declared actions (a JSON array). `codes` are the codes of the connector's own announcement
/// catalog: they become the `enum` of `test.announce`'s `code` param.
pub fn standard_actions(codes: &[&str]) -> Value {
    json!([
        {
            "code": TEST_PING,
            "description": "Radio round trip: the connector answers and says its radio check on the user's radio. Use it when the user asks for a radio test, or to prove that the connector is alive.",
            "timeout_s": 5,
            "params": {"type": "object", "additionalProperties": false, "properties": {
                "say": {"type": "boolean",
                        "description": "Also play the radio check on the user's radio (default true)."}}},
            "result": {"type": "object", "additionalProperties": false,
                       "required": ["pong", "announced"], "properties": {
                "pong": {"type": "string", "maxLength": 64, "description": "Always 'pong'."},
                "announced": {"type": "boolean",
                              "description": "Pitwall accepted the radio check announcement."},
                "message_id": {"type": "string", "maxLength": 64,
                               "description": "The announcement's id (list_connector_messages), when announced."}}},
            "example": {"say": true},
        },
        {
            "code": TEST_ANNOUNCE,
            "description": "For desk tests of the radio calls: sends one announcement of the connector's own catalog with the given params; the user hears the codes whose audience is driver. Param values are text and are converted by the code's param kinds (a lap time in seconds, a count as digits); a missing or unknown param is refused with bad_request naming it.",
            "timeout_s": 5,
            "params": {"type": "object", "additionalProperties": false,
                       "required": ["code"], "properties": {
                "code": {"type": "string", "enum": codes,
                         "description": "The catalog code to announce (list_connector_messages)."},
                "params": {"type": "array", "maxItems": 8,
                           "description": "The code's params, one entry per param it declares.",
                           "items": {"type": "object", "additionalProperties": false,
                                     "required": ["name", "value"],
                                     "description": "One param of the announcement.",
                                     "properties": {
                               "name": {"type": "string", "maxLength": 32,
                                        "description": "The param's name, as the catalog declares it."},
                               "value": {"type": "string", "maxLength": 200,
                                         "description": "The value as text."}}}},
                "variant": {"type": "integer", "minimum": 0, "maximum": 7,
                            "description": "Which wording of the code to send: 0 (default) its text, 1.. its variants (list_connector_messages); beyond the code's variants it is refused."}}},
            "result": {"type": "object", "additionalProperties": false,
                       "required": ["announced"], "properties": {
                "announced": {"type": "boolean",
                              "description": "Pitwall accepted the announcement."},
                "message_id": {"type": "string", "maxLength": 64,
                               "description": "The announcement's id, when announced."},
                "refused": {"type": "string", "maxLength": 300,
                            "description": "Why it was not announced (rate limit, Pitwall's refusal), when it was not."}}},
            "example": {"code": "engineer_test",
                        "params": [{"name": "text", "value": "radio test"}]},
        },
    ])
}

/// The idempotency key of the declaration: `<prefix>/actions/<sha12 of the actions>`.
pub fn actions_ik(prefix: &str, actions: &Value) -> String {
    format!("{prefix}/actions/{}", sha12(&canonical(actions)))
}
