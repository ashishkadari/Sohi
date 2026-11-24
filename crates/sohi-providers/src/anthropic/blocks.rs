//! Content blocks: the conversation as Anthropic wants it, and putting a
//! streamed reply back together.
//!
//! A reply that calls tools has to be sent back as it came, thinking
//! blocks included, together with the results. Its blocks are therefore
//! kept as JSON in [`Message::blocks`].

use serde_json::{Map, Value, json};
use sohi_core::chat::{Message, Role, ToolCall, ToolSpec};

use crate::{Completion, ProviderError};

/// The `messages` of a request. Tool results in a row become one user
/// message, as the API wants them.
pub(super) fn messages(messages: &[Message]) -> Vec<Value> {
    let mut wire: Vec<Value> = Vec::with_capacity(messages.len());
    let mut results: Vec<Value> = Vec::new();
    for message in messages {
        if message.role == Role::Tool {
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": message.call_id,
                "content": message.content,
            }));
            continue;
        }
        flush(&mut results, &mut wire);
        let content = if message.calls.is_empty() {
            json!(message.content)
        } else {
            call_blocks(message)
        };
        wire.push(json!({ "role": message.role.as_str(), "content": content }));
    }
    flush(&mut results, &mut wire);
    wire
}

fn flush(results: &mut Vec<Value>, wire: &mut Vec<Value>) {
    if !results.is_empty() {
        wire.push(json!({ "role": "user", "content": std::mem::take(results) }));
    }
}

/// The blocks of a message that calls tools: as they came, or made from
/// the calls if another provider wrote them.
fn call_blocks(message: &Message) -> Value {
    if let Some(Ok(blocks)) = message.blocks.as_deref().map(serde_json::from_str) {
        return blocks;
    }
    let mut blocks = Vec::new();
    if !message.content.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": message.content }));
    }
    for call in &message.calls {
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": input(&call.arguments),
        }));
    }
    Value::Array(blocks)
}

/// A call's arguments as the object they must be.
fn input(arguments: &str) -> Value {
    match serde_json::from_str(arguments) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => Value::Object(Map::new()),
    }
}

/// The `tools` of a request.
pub(super) fn tools(specs: &[ToolSpec]) -> Result<Vec<Value>, ProviderError> {
    specs
        .iter()
        .map(|spec| {
            let schema: Value = serde_json::from_str(spec.parameters)
                .map_err(|e| ProviderError::BadResponse(format!("tool schema: {e}")))?;
            Ok(json!({
                "name": spec.name,
                "description": spec.description,
                "input_schema": schema,
            }))
        })
        .collect()
}

/// A reply being put back together.
#[derive(Default)]
pub(super) struct Blocks {
    blocks: Vec<Block>,
}

struct Block {
    index: u64,
    value: Value,
    /// The JSON of a tool's input so far.
    input: String,
}

impl Blocks {
    /// A block starts.
    pub fn start(&mut self, index: u64, block: Value) {
        self.blocks.push(Block {
            index,
            value: block,
            input: String::new(),
        });
    }

    /// More of a block. Deltas for blocks that never started are ignored.
    pub fn push(&mut self, index: u64, delta: &Value) {
        let Some(block) = self.blocks.iter_mut().find(|b| b.index == index) else {
            return;
        };
        let part = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
        match part("type") {
            "text_delta" => append(&mut block.value, "text", part("text")),
            "thinking_delta" => append(&mut block.value, "thinking", part("thinking")),
            "signature_delta" => append(&mut block.value, "signature", part("signature")),
            "input_json_delta" => block.input.push_str(part("partial_json")),
            _ => {}
        }
    }

    /// The tool calls, if there are any, and everything to send back with
    /// their results.
    pub fn finish(self) -> Completion {
        let mut calls = Vec::new();
        let mut values = Vec::new();
        for block in self.blocks {
            let mut value = block.value;
            let field = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default();
            if field("type") == "text" && field("text").is_empty() {
                continue;
            }
            if field("type") == "tool_use" {
                let arguments = block.input;
                calls.push(ToolCall {
                    id: field("id").to_owned(),
                    name: field("name").to_owned(),
                    arguments: arguments.clone(),
                });
                if let Some(object) = value.as_object_mut() {
                    object.insert("input".to_owned(), input(&arguments));
                }
            }
            values.push(value);
        }
        if calls.is_empty() {
            return Completion::default();
        }
        Completion {
            calls,
            blocks: Some(Value::Array(values).to_string()),
        }
    }
}

fn append(value: &mut Value, key: &str, more: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let entry = object.entry(key).or_insert_with(|| json!(""));
    if let Value::String(text) = entry {
        text.push_str(more);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> ToolCall {
        ToolCall {
            id: "toolu_1".to_owned(),
            name: "volume".to_owned(),
            arguments: r#"{"action":"up"}"#.to_owned(),
        }
    }

    #[test]
    fn results_share_one_user_message() {
        let wire = messages(&[
            Message::user("hi"),
            Message::calls("Sure.", vec![call(), call()], None),
            Message::tool_result("toolu_1", "a"),
            Message::tool_result("toolu_2", "b"),
            Message::assistant("Done."),
        ]);
        let roles: Vec<_> = wire.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(wire[1]["content"][0]["text"], "Sure.");
        assert_eq!(wire[1]["content"][1]["input"]["action"], "up");
        assert_eq!(wire[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(wire[2]["content"][1]["content"], "b");
    }

    #[test]
    fn blocks_are_sent_back_as_they_came() {
        let mut blocks = Blocks::default();
        blocks.start(0, json!({"type":"thinking","thinking":""}));
        blocks.push(0, &json!({"type":"thinking_delta","thinking":"hmm"}));
        blocks.push(0, &json!({"type":"signature_delta","signature":"sig"}));
        blocks.start(1, json!({"type":"text","text":""}));
        blocks.start(
            2,
            json!({"type":"tool_use","id":"toolu_9","name":"volume","input":{}}),
        );
        blocks.push(2, &json!({"type":"input_json_delta","partial_json":""}));
        blocks.push(
            2,
            &json!({"type":"input_json_delta","partial_json":"{\"action\":"}),
        );
        blocks.push(
            2,
            &json!({"type":"input_json_delta","partial_json":"\"up\"}"}),
        );
        blocks.push(7, &json!({"type":"text_delta","text":"lost"}));
        let done = blocks.finish();
        assert_eq!(done.calls.len(), 1);
        assert_eq!(done.calls[0].arguments, r#"{"action":"up"}"#);
        let message = Message::calls("", done.calls, done.blocks);
        let wire = messages(&[message]);
        assert_eq!(
            wire[0]["content"],
            json!([
                {"type":"thinking","thinking":"hmm","signature":"sig"},
                {"type":"tool_use","id":"toolu_9","name":"volume","input":{"action":"up"}},
            ])
        );
    }

    #[test]
    fn a_reply_without_calls_needs_no_blocks() {
        let mut blocks = Blocks::default();
        blocks.start(0, json!({"type":"text","text":""}));
        blocks.push(0, &json!({"type":"text_delta","text":"Hi."}));
        assert_eq!(blocks.finish(), Completion::default());
    }
}
