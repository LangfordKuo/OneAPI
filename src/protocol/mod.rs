use serde_json::{Map, Value};

pub mod anthropic;
pub mod chat;
pub mod error;
pub mod ir;
pub mod responses;

pub use error::ProtocolError;
pub use ir::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Chat,
    Responses,
    Anthropic,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn parse_request(self, value: &Value) -> Result<UnifiedRequest, ProtocolError> {
        match self {
            Self::Chat => chat::parse_request(value),
            Self::Responses => responses::parse_request(value),
            Self::Anthropic => anthropic::parse_request(value),
        }
    }

    pub fn encode_request(self, request: &UnifiedRequest) -> Result<Value, ProtocolError> {
        match self {
            Self::Chat => chat::encode_request(request),
            Self::Responses => responses::encode_request(request),
            Self::Anthropic => anthropic::encode_request(request),
        }
    }

    pub fn parse_response(self, value: &Value) -> Result<UnifiedResponse, ProtocolError> {
        match self {
            Self::Chat => chat::parse_response(value),
            Self::Responses => responses::parse_response(value),
            Self::Anthropic => anthropic::parse_response(value),
        }
    }

    pub fn encode_response(self, response: &UnifiedResponse) -> Result<Value, ProtocolError> {
        match self {
            Self::Chat => chat::encode_response(response),
            Self::Responses => responses::encode_response(response),
            Self::Anthropic => anthropic::encode_response(response),
        }
    }
}

pub fn convert_request(
    from: Protocol,
    to: Protocol,
    value: &Value,
) -> Result<Value, ProtocolError> {
    let request = from.parse_request(value)?;
    to.encode_request(&request)
}

pub fn convert_response(
    from: Protocol,
    to: Protocol,
    value: &Value,
) -> Result<Value, ProtocolError> {
    let response = from.parse_response(value)?;
    to.encode_response(&response)
}

pub(crate) fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

pub(crate) fn get_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    get_str(obj, key).map(str::to_string)
}

pub(crate) fn get_u32(obj: &Map<String, Value>, key: &str) -> Option<u32> {
    obj.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn get_u64(obj: &Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(Value::as_u64)
}

pub(crate) fn get_f64(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

pub(crate) fn get_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(Value::as_bool)
}

pub(crate) fn parse_arguments(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

pub(crate) fn parse_stop(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn parse_effort(value: &str) -> ReasoningEffort {
    match value {
        "minimal" => ReasoningEffort::Minimal,
        "low" => ReasoningEffort::Low,
        "high" => ReasoningEffort::High,
        _ => ReasoningEffort::Medium,
    }
}

pub(crate) fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

pub(crate) fn budget_for_effort(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Minimal => 1024,
        ReasoningEffort::Low => 2048,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High => 16384,
    }
}

pub(crate) fn effort_for_budget(budget: u32) -> ReasoningEffort {
    if budget <= 1024 {
        ReasoningEffort::Minimal
    } else if budget < 8192 {
        ReasoningEffort::Low
    } else if budget < 16384 {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::High
    }
}

pub(crate) fn reject_extras(
    request: &UnifiedRequest,
    target: &'static str,
    fields: &[&str],
) -> Result<(), ProtocolError> {
    for field in fields {
        if request.extras.contains_key(*field) {
            return Err(ProtocolError::unsupported(*field, target));
        }
    }
    Ok(())
}

pub(crate) fn ensure_text_only(
    parts: &[ContentPart],
    target: &'static str,
) -> Result<(), ProtocolError> {
    if parts
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        return Err(ProtocolError::unsupported("image in tool_result", target));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat_request() -> Value {
        json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": [
                    {"type": "text", "text": "What is the weather?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Paris\"}"
                    }}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            }}],
            "tool_choice": {"type": "function", "function": {"name": "get_weather"}},
            "max_tokens": 4096,
            "temperature": 0.2,
            "stop": ["END"],
            "reasoning_effort": "low",
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "schema": {"type": "object"},
                    "strict": true
                }
            },
            "user": "user-1",
            "parallel_tool_calls": false
        })
    }

    fn responses_request() -> Value {
        json!({
            "model": "gpt-4o",
            "instructions": "Be concise.",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "Hello"},
                    {"type": "input_image", "image_url": "data:image/jpeg;base64,BBBB"}
                ]},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "thinking"}], "encrypted_content": "sig"},
                {"type": "function_call", "call_id": "call_2", "name": "lookup", "arguments": "{\"q\":\"x\"}"},
                {"type": "function_call_output", "call_id": "call_2", "output": "result"}
            ],
            "tools": [{"type": "function", "name": "lookup", "description": "Lookup", "parameters": {"type": "object"}, "strict": true}],
            "tool_choice": {"type": "function", "name": "lookup"},
            "max_output_tokens": 4096,
            "temperature": 0.5,
            "reasoning": {"effort": "high", "summary": "auto"},
            "text": {"format": {"type": "json_schema", "name": "out", "schema": {"type": "object"}, "strict": true}},
            "store": true,
            "previous_response_id": "resp_prev",
            "parallel_tool_calls": false
        })
    }

    fn anthropic_request() -> Value {
        json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 2048,
            "system": [{"type": "text", "text": "You are Claude."}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "Hi"},
                    {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "Let me think", "signature": "sig"},
                    {"type": "text", "text": "Calling tool"},
                    {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "rust"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "found"}
                ]}
            ],
            "tools": [{"name": "search", "description": "Search", "input_schema": {"type": "object"}}],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
            "temperature": 0.7,
            "top_p": 0.9,
            "stop_sequences": ["STOP"],
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "metadata": {"user_id": "u1"},
            "output_format": {"type": "json_schema", "schema": {"type": "object"}}
        })
    }

    fn chat_response() -> Value {
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 123,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                    "reasoning_content": "think",
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "f", "arguments": "{\"x\":1}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": {"cached_tokens": 10},
                "completion_tokens_details": {"reasoning_tokens": 5}
            }
        })
    }

    fn responses_response() -> Value {
        json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 456,
            "status": "completed",
            "model": "gpt-4o",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "plan"}], "encrypted_content": "enc"},
                {"type": "message", "id": "msg_1", "role": "assistant", "content": [{"type": "output_text", "text": "Done", "annotations": []}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "f", "arguments": "{\"x\":1}"}
            ],
            "usage": {
                "input_tokens": 80,
                "output_tokens": 15,
                "total_tokens": 95,
                "input_tokens_details": {"cached_tokens": 8},
                "output_tokens_details": {"reasoning_tokens": 3}
            }
        })
    }

    fn anthropic_response() -> Value {
        json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "text", "text": "Hi"},
                {"type": "tool_use", "id": "toolu_1", "name": "f", "input": {"x": 1}}
            ],
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10,
                "cache_read_input_tokens": 5,
                "cache_creation_input_tokens": 2
            }
        })
    }

    #[test]
    fn chat_to_anthropic_request() {
        let converted =
            convert_request(Protocol::Chat, Protocol::Anthropic, &chat_request()).unwrap();
        assert_eq!(converted["model"], json!("gpt-4o"));
        assert_eq!(converted["system"], json!("You are helpful."));
        assert_eq!(converted["max_tokens"], json!(4096));
        assert_eq!(converted["stop_sequences"], json!(["END"]));
        assert_eq!(converted["temperature"], json!(0.2));
        assert_eq!(converted["tool_choice"]["type"], json!("tool"));
        assert_eq!(converted["tool_choice"]["name"], json!("get_weather"));
        assert_eq!(
            converted["tool_choice"]["disable_parallel_tool_use"],
            json!(true)
        );
        assert_eq!(
            converted["tools"][0]["input_schema"]["type"],
            json!("object")
        );
        assert_eq!(converted["thinking"]["budget_tokens"], json!(2048));
        assert_eq!(converted["output_format"]["type"], json!("json_schema"));

        let messages = converted["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(
            messages[0]["content"][0]["text"],
            json!("What is the weather?")
        );
        assert_eq!(messages[0]["content"][1]["type"], json!("image"));
        assert_eq!(messages[1]["content"][0]["type"], json!("tool_use"));
        assert_eq!(messages[1]["content"][0]["name"], json!("get_weather"));
        assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
        assert_eq!(messages[2]["content"][0]["content"], json!("sunny"));
    }

    #[test]
    fn chat_to_responses_request() {
        let mut request = chat_request();
        request.as_object_mut().unwrap().remove("stop");
        let converted = convert_request(Protocol::Chat, Protocol::Responses, &request).unwrap();
        assert_eq!(converted["instructions"], json!("You are helpful."));
        assert_eq!(converted["max_output_tokens"], json!(4096));
        assert_eq!(converted["store"], json!(false));
        assert_eq!(converted["parallel_tool_calls"], json!(false));
        assert_eq!(converted["reasoning"]["effort"], json!("low"));
        assert_eq!(converted["text"]["format"]["type"], json!("json_schema"));
        assert_eq!(converted["tools"][0]["name"], json!("get_weather"));
        assert_eq!(converted["tool_choice"]["type"], json!("function"));
        assert_eq!(converted["tool_choice"]["name"], json!("get_weather"));

        let input = converted["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["content"][1]["type"], json!("input_image"));
        assert_eq!(input[1]["type"], json!("function_call"));
        assert_eq!(input[1]["call_id"], json!("call_1"));
        assert_eq!(input[2]["type"], json!("function_call_output"));
        assert_eq!(input[2]["call_id"], json!("call_1"));
    }

    #[test]
    fn responses_to_chat_request() {
        let converted =
            convert_request(Protocol::Responses, Protocol::Chat, &responses_request()).unwrap();
        assert_eq!(converted["messages"][0]["role"], json!("system"));
        assert_eq!(converted["messages"][0]["content"], json!("Be concise."));
        assert_eq!(converted["max_tokens"], json!(4096));
        assert_eq!(converted["temperature"], json!(0.5));
        assert_eq!(converted["reasoning_effort"], json!("high"));
        assert_eq!(converted["response_format"]["type"], json!("json_schema"));
        assert_eq!(converted["tools"][0]["function"]["name"], json!("lookup"));
        assert_eq!(
            converted["tool_choice"]["function"]["name"],
            json!("lookup")
        );
        assert_eq!(converted["parallel_tool_calls"], json!(false));

        let messages = converted["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"][1]["type"], json!("image_url"));
        assert_eq!(messages[2]["role"], json!("assistant"));
        assert_eq!(messages[2]["reasoning_content"], json!("thinking"));
        assert_eq!(messages[2]["tool_calls"][0]["id"], json!("call_2"));
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["name"],
            json!("lookup")
        );
        assert_eq!(messages[3]["role"], json!("tool"));
        assert_eq!(messages[3]["content"], json!("result"));
    }

    #[test]
    fn responses_to_anthropic_request() {
        let converted = convert_request(
            Protocol::Responses,
            Protocol::Anthropic,
            &responses_request(),
        )
        .unwrap();
        assert_eq!(converted["system"], json!("Be concise."));
        assert_eq!(converted["max_tokens"], json!(4096));
        assert_eq!(converted["tool_choice"]["type"], json!("tool"));
        assert_eq!(converted["tool_choice"]["name"], json!("lookup"));
        assert_eq!(
            converted["tool_choice"]["disable_parallel_tool_use"],
            json!(true)
        );
        assert_eq!(converted["thinking"]["budget_tokens"], json!(4095));
        assert_eq!(converted["output_format"]["type"], json!("json_schema"));

        let messages = converted["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"][1]["type"], json!("image"));
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"][0]["type"], json!("thinking"));
        assert_eq!(messages[1]["content"][0]["signature"], json!("sig"));
        assert_eq!(messages[1]["content"][1]["type"], json!("tool_use"));
        assert_eq!(messages[2]["role"], json!("user"));
        assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
    }

    #[test]
    fn anthropic_to_chat_request() {
        let converted =
            convert_request(Protocol::Anthropic, Protocol::Chat, &anthropic_request()).unwrap();
        assert_eq!(converted["messages"][0]["role"], json!("system"));
        assert_eq!(
            converted["messages"][0]["content"],
            json!("You are Claude.")
        );
        assert_eq!(converted["max_tokens"], json!(2048));
        assert_eq!(converted["temperature"], json!(0.7));
        assert_eq!(converted["top_p"], json!(0.9));
        assert_eq!(converted["stop"], json!(["STOP"]));
        assert_eq!(converted["reasoning_effort"], json!("minimal"));
        assert_eq!(converted["response_format"]["type"], json!("json_schema"));
        assert_eq!(converted["user"], json!("u1"));
        assert_eq!(converted["tools"][0]["function"]["name"], json!("search"));
        assert_eq!(converted["parallel_tool_calls"], json!(false));

        let messages = converted["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"][1]["type"], json!("image_url"));
        assert_eq!(messages[2]["role"], json!("assistant"));
        assert_eq!(messages[2]["reasoning_content"], json!("Let me think"));
        assert_eq!(messages[2]["content"], json!("Calling tool"));
        assert_eq!(messages[2]["tool_calls"][0]["id"], json!("toolu_1"));
        assert_eq!(messages[3]["role"], json!("tool"));
        assert_eq!(messages[3]["content"], json!("found"));
    }

    #[test]
    fn anthropic_to_responses_request() {
        let mut request = anthropic_request();
        request.as_object_mut().unwrap().remove("stop_sequences");
        let converted =
            convert_request(Protocol::Anthropic, Protocol::Responses, &request).unwrap();
        assert_eq!(converted["instructions"], json!("You are Claude."));
        assert_eq!(converted["max_output_tokens"], json!(2048));
        assert_eq!(converted["parallel_tool_calls"], json!(false));
        assert_eq!(converted["reasoning"]["effort"], json!("minimal"));
        assert_eq!(converted["text"]["format"]["type"], json!("json_schema"));
        assert_eq!(converted["store"], json!(false));

        let input = converted["input"].as_array().unwrap();
        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["content"][1]["type"], json!("input_image"));
        assert_eq!(input[1]["type"], json!("message"));
        assert_eq!(input[1]["role"], json!("assistant"));
        assert_eq!(input[1]["content"][0]["text"], json!("Calling tool"));
        assert_eq!(input[2]["type"], json!("function_call"));
        assert_eq!(input[2]["call_id"], json!("toolu_1"));
        assert_eq!(input[3]["type"], json!("function_call_output"));
        assert_eq!(input[3]["output"], json!("found"));
    }

    #[test]
    fn chat_round_trip_keeps_core_fields() {
        let request = chat_request();
        let ir = Protocol::Chat.parse_request(&request).unwrap();
        let encoded = Protocol::Chat.encode_request(&ir).unwrap();
        assert_eq!(encoded["messages"][0]["role"], json!("system"));
        assert_eq!(encoded["max_tokens"], json!(4096));
        assert_eq!(encoded["stop"], json!(["END"]));
        assert_eq!(encoded["reasoning_effort"], json!("low"));
        assert_eq!(encoded["response_format"]["type"], json!("json_schema"));
        assert_eq!(
            encoded["tool_choice"]["function"]["name"],
            json!("get_weather")
        );
        assert_eq!(encoded["messages"][3]["tool_call_id"], json!("call_1"));
    }

    #[test]
    fn responses_round_trip_keeps_core_fields() {
        let request = responses_request();
        let ir = Protocol::Responses.parse_request(&request).unwrap();
        assert_eq!(ir.previous_response_id.as_deref(), Some("resp_prev"));
        let encoded = Protocol::Responses.encode_request(&ir).unwrap();
        assert_eq!(encoded["instructions"], json!("Be concise."));
        assert_eq!(encoded["store"], json!(false));
        assert_eq!(encoded["text"]["format"]["type"], json!("json_schema"));
        assert_eq!(encoded["input"].as_array().unwrap().len(), 3);
        assert_eq!(encoded["input"][1]["type"], json!("function_call"));
        assert_eq!(encoded["input"][2]["type"], json!("function_call_output"));
    }

    #[test]
    fn anthropic_round_trip_keeps_core_fields() {
        let request = anthropic_request();
        let ir = Protocol::Anthropic.parse_request(&request).unwrap();
        assert_eq!(ir.max_tokens, Some(2048));
        let encoded = Protocol::Anthropic.encode_request(&ir).unwrap();
        assert_eq!(encoded["system"], json!("You are Claude."));
        assert_eq!(encoded["max_tokens"], json!(2048));
        assert_eq!(encoded["stop_sequences"], json!(["STOP"]));
        assert_eq!(encoded["thinking"]["budget_tokens"], json!(1024));
        assert_eq!(encoded["output_format"]["type"], json!("json_schema"));
        assert_eq!(
            encoded["messages"][1]["content"][0]["type"],
            json!("thinking")
        );
    }

    #[test]
    fn chat_response_to_anthropic() {
        let converted =
            convert_response(Protocol::Chat, Protocol::Anthropic, &chat_response()).unwrap();
        assert_eq!(converted["type"], json!("message"));
        assert_eq!(converted["stop_reason"], json!("tool_use"));
        let content = converted["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], json!("text"));
        assert_eq!(content[0]["text"], json!("Hello"));
        assert_eq!(content[1]["type"], json!("tool_use"));
        assert_eq!(converted["usage"]["input_tokens"], json!(90));
        assert_eq!(converted["usage"]["cache_read_input_tokens"], json!(10));
        assert_eq!(converted["usage"]["output_tokens"], json!(20));
    }

    #[test]
    fn chat_response_to_responses() {
        let converted =
            convert_response(Protocol::Chat, Protocol::Responses, &chat_response()).unwrap();
        assert_eq!(converted["status"], json!("completed"));
        let output = converted["output"].as_array().unwrap();
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[0]["summary"][0]["text"], json!("think"));
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["content"][0]["text"], json!("Hello"));
        assert_eq!(output[2]["type"], json!("function_call"));
        assert_eq!(converted["usage"]["input_tokens"], json!(100));
        assert_eq!(
            converted["usage"]["input_tokens_details"]["cached_tokens"],
            json!(10)
        );
    }

    #[test]
    fn anthropic_response_to_chat() {
        let converted =
            convert_response(Protocol::Anthropic, Protocol::Chat, &anthropic_response()).unwrap();
        let message = &converted["choices"][0]["message"];
        assert_eq!(message["content"], json!("Hi"));
        assert_eq!(message["reasoning_content"], json!("hmm"));
        assert_eq!(message["tool_calls"][0]["id"], json!("toolu_1"));
        assert_eq!(
            converted["choices"][0]["finish_reason"],
            json!("tool_calls")
        );
        assert_eq!(converted["usage"]["prompt_tokens"], json!(57));
        assert_eq!(converted["usage"]["completion_tokens"], json!(10));
        assert_eq!(
            converted["usage"]["prompt_tokens_details"]["cached_tokens"],
            json!(5)
        );
    }

    #[test]
    fn anthropic_response_to_responses() {
        let converted = convert_response(
            Protocol::Anthropic,
            Protocol::Responses,
            &anthropic_response(),
        )
        .unwrap();
        let output = converted["output"].as_array().unwrap();
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[0]["encrypted_content"], json!("sig"));
        assert_eq!(output[1]["content"][0]["text"], json!("Hi"));
        assert_eq!(output[2]["type"], json!("function_call"));
        assert_eq!(converted["usage"]["input_tokens"], json!(57));
        assert_eq!(converted["usage"]["output_tokens"], json!(10));
    }

    #[test]
    fn responses_response_to_chat() {
        let converted =
            convert_response(Protocol::Responses, Protocol::Chat, &responses_response()).unwrap();
        let message = &converted["choices"][0]["message"];
        assert_eq!(message["content"], json!("Done"));
        assert_eq!(message["reasoning_content"], json!("plan"));
        assert_eq!(message["tool_calls"][0]["id"], json!("call_1"));
        assert_eq!(converted["usage"]["prompt_tokens"], json!(80));
        assert_eq!(converted["usage"]["completion_tokens"], json!(15));
        assert_eq!(
            converted["usage"]["prompt_tokens_details"]["cached_tokens"],
            json!(8)
        );
        assert_eq!(
            converted["usage"]["completion_tokens_details"]["reasoning_tokens"],
            json!(3)
        );
    }

    #[test]
    fn responses_response_to_anthropic() {
        let converted = convert_response(
            Protocol::Responses,
            Protocol::Anthropic,
            &responses_response(),
        )
        .unwrap();
        let content = converted["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], json!("thinking"));
        assert_eq!(content[0]["signature"], json!("enc"));
        assert_eq!(content[1]["type"], json!("text"));
        assert_eq!(content[2]["type"], json!("tool_use"));
        assert_eq!(converted["usage"]["input_tokens"], json!(72));
        assert_eq!(converted["usage"]["cache_read_input_tokens"], json!(8));
        assert_eq!(converted["usage"]["output_tokens"], json!(15));
    }

    #[test]
    fn chat_n_to_anthropic_is_rejected() {
        let mut request = chat_request();
        request["n"] = json!(2);
        let error = convert_request(Protocol::Chat, Protocol::Anthropic, &request).unwrap_err();
        assert!(matches!(error, ProtocolError::UnsupportedField { .. }));
    }

    #[test]
    fn chat_stop_to_responses_is_rejected() {
        let error =
            convert_request(Protocol::Chat, Protocol::Responses, &chat_request()).unwrap_err();
        assert!(matches!(error, ProtocolError::UnsupportedField { .. }));
    }

    #[test]
    fn anthropic_top_k_to_chat_is_rejected() {
        let mut request = anthropic_request();
        request["top_k"] = json!(40);
        let error = convert_request(Protocol::Anthropic, Protocol::Chat, &request).unwrap_err();
        assert!(matches!(error, ProtocolError::UnsupportedField { .. }));
    }

    #[test]
    fn anthropic_stop_to_responses_is_rejected() {
        let error = convert_request(
            Protocol::Anthropic,
            Protocol::Responses,
            &anthropic_request(),
        )
        .unwrap_err();
        assert!(matches!(error, ProtocolError::UnsupportedField { .. }));
    }

    #[test]
    fn json_object_to_anthropic_is_rejected() {
        let mut request = chat_request();
        request["response_format"] = json!({"type": "json_object"});
        let error = convert_request(Protocol::Chat, Protocol::Anthropic, &request).unwrap_err();
        assert!(matches!(error, ProtocolError::UnsupportedField { .. }));
    }
}
