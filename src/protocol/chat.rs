use serde_json::{json, Map, Value};

use super::{
    effort_for_budget, effort_name, ensure_text_only, get_bool, get_f64, get_str, get_string,
    get_u32, get_u64, image_url, parse_arguments, parse_effort, parse_image_string, parse_stop,
    reasoning_text, text_content, ContentPart, ProtocolError, ReasoningConfig, ResponseFormat,
    Role, StopReason, ToolChoice, ToolDef, UnifiedMessage, UnifiedRequest, UnifiedResponse, Usage,
};

pub fn parse_request(value: &Value) -> Result<UnifiedRequest, ProtocolError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ProtocolError::invalid_request("chat", "request must be a JSON object"))?;

    let model = get_str(obj, "model")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProtocolError::invalid_request("chat", "model is required"))?
        .to_string();

    let messages_value = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProtocolError::invalid_request("chat", "messages must be an array"))?;

    let mut system = Vec::new();
    let mut messages = Vec::new();

    for (index, message) in messages_value.iter().enumerate() {
        let message = message.as_object().ok_or_else(|| {
            ProtocolError::invalid_request("chat", format!("messages[{index}] must be an object"))
        })?;
        let role = get_str(message, "role").ok_or_else(|| {
            ProtocolError::invalid_request("chat", format!("messages[{index}].role is required"))
        })?;

        match role {
            "system" | "developer" => {
                system.extend(parse_content(message.get("content"), false, "chat")?);
            }
            "user" => {
                messages.push(UnifiedMessage {
                    role: Role::User,
                    content: parse_content(message.get("content"), true, "chat")?,
                });
            }
            "assistant" => {
                let mut content = parse_content(message.get("content"), true, "chat")?;
                if let Some(reasoning) =
                    get_str(message, "reasoning_content").or_else(|| get_str(message, "reasoning"))
                {
                    if !reasoning.is_empty() {
                        content.insert(
                            0,
                            ContentPart::Reasoning {
                                text: reasoning.to_string(),
                                signature: None,
                            },
                        );
                    }
                }

                if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for call in tool_calls {
                        let call = call.as_object().ok_or_else(|| {
                            ProtocolError::invalid_request("chat", "tool_call must be an object")
                        })?;
                        let id = get_str(call, "id").unwrap_or_default().to_string();
                        let function =
                            call.get("function")
                                .and_then(Value::as_object)
                                .ok_or_else(|| {
                                    ProtocolError::invalid_request(
                                        "chat",
                                        "tool_call.function must be an object",
                                    )
                                })?;
                        let name = get_str(function, "name")
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| {
                                ProtocolError::invalid_request(
                                    "chat",
                                    "tool_call.function.name is required",
                                )
                            })?
                            .to_string();
                        let input = parse_arguments(get_str(function, "arguments").unwrap_or("{}"));
                        content.push(ContentPart::ToolUse { id, name, input });
                    }
                }

                messages.push(UnifiedMessage {
                    role: Role::Assistant,
                    content,
                });
            }
            "tool" => {
                let tool_use_id = get_str(message, "tool_call_id")
                    .ok_or_else(|| {
                        ProtocolError::invalid_request("chat", "tool message requires tool_call_id")
                    })?
                    .to_string();
                let content = parse_content(message.get("content"), false, "chat")?;
                messages.push(UnifiedMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        is_error: false,
                    }],
                });
            }
            other => {
                return Err(ProtocolError::invalid_request(
                    "chat",
                    format!("unsupported message role `{other}`"),
                ))
            }
        }
    }

    let mut tools = Vec::new();
    if let Some(values) = obj.get("tools").and_then(Value::as_array) {
        for tool in values {
            let tool = tool
                .as_object()
                .ok_or_else(|| ProtocolError::invalid_request("chat", "tool must be an object"))?;
            if get_str(tool, "type").unwrap_or("function") != "function" {
                continue;
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ProtocolError::invalid_request("chat", "tool.function must be an object")
                })?;
            let name = get_str(function, "name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ProtocolError::invalid_request("chat", "tool name is required"))?
                .to_string();
            tools.push(ToolDef {
                name,
                description: get_string(function, "description"),
                parameters: function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                strict: get_bool(function, "strict"),
            });
        }
    }

    let tool_choice = parse_tool_choice(obj.get("tool_choice"))?;
    let max_tokens = get_u32(obj, "max_completion_tokens").or_else(|| get_u32(obj, "max_tokens"));
    let reasoning = get_str(obj, "reasoning_effort").map(|effort| ReasoningConfig {
        effort: Some(parse_effort(effort)),
        budget_tokens: None,
        summary: None,
    });
    let response_format = parse_response_format(obj.get("response_format"))?;

    let mut extras = Map::new();
    for key in [
        "logit_bias",
        "presence_penalty",
        "frequency_penalty",
        "seed",
        "audio",
        "modalities",
    ] {
        if let Some(value) = obj.get(key) {
            extras.insert(key.to_string(), value.clone());
        }
    }

    Ok(UnifiedRequest {
        model,
        system,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls: get_bool(obj, "parallel_tool_calls"),
        max_tokens,
        temperature: get_f64(obj, "temperature"),
        top_p: get_f64(obj, "top_p"),
        top_k: None,
        stop: parse_stop(obj.get("stop")),
        stream: get_bool(obj, "stream").unwrap_or(false),
        reasoning,
        response_format,
        metadata_user_id: get_string(obj, "user"),
        previous_response_id: None,
        store: None,
        n: get_u32(obj, "n"),
        extras,
    })
}

fn parse_content(
    value: Option<&Value>,
    allow_images: bool,
    protocol: &'static str,
) -> Result<Vec<ContentPart>, ProtocolError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Value::Array(parts) => {
            let mut result = Vec::new();
            for part in parts {
                let part = part.as_object().ok_or_else(|| {
                    ProtocolError::invalid_request(protocol, "content part must be an object")
                })?;
                match get_str(part, "type").unwrap_or("text") {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = get_str(part, "text") {
                            result.push(ContentPart::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    "image_url" | "input_image" => {
                        if !allow_images {
                            return Err(ProtocolError::unsupported(
                                "image in system/tool content",
                                protocol,
                            ));
                        }
                        let source = if let Some(url) = get_str(part, "image_url") {
                            Some(parse_image_string(url))
                        } else if let Some(image) = part.get("image_url").and_then(Value::as_object)
                        {
                            get_str(image, "url").map(parse_image_string)
                        } else {
                            get_str(part, "url").map(parse_image_string)
                        };
                        if let Some(source) = source {
                            result.push(ContentPart::Image { source });
                        }
                    }
                    _ => {}
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_request(
            protocol,
            "content must be a string or array",
        )),
    }
}

fn parse_output_content(value: Option<&Value>) -> Result<Vec<ContentPart>, ProtocolError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Some(Value::Array(parts)) => {
            let mut result = Vec::new();
            for part in parts {
                let part = part.as_object().ok_or_else(|| {
                    ProtocolError::invalid_response("chat", "content part must be an object")
                })?;
                if matches!(
                    get_str(part, "type").unwrap_or("text"),
                    "text" | "output_text"
                ) {
                    if let Some(text) = get_str(part, "text") {
                        result.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_response(
            "chat",
            "message content must be a string or array",
        )),
    }
}

fn parse_tool_choice(value: Option<&Value>) -> Result<ToolChoice, ProtocolError> {
    match value {
        None => Ok(ToolChoice::Auto),
        Some(Value::String(value)) => match value.as_str() {
            "auto" => Ok(ToolChoice::Auto),
            "none" => Ok(ToolChoice::None),
            "required" => Ok(ToolChoice::Required),
            other => Err(ProtocolError::invalid_request(
                "chat",
                format!("unsupported tool_choice `{other}`"),
            )),
        },
        Some(Value::Object(value)) => {
            let kind = get_str(value, "type").unwrap_or_default();
            let name = value
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| get_str(function, "name"))
                .or_else(|| get_str(value, "name"));
            match kind {
                "function" | "tool" => Ok(ToolChoice::Tool {
                    name: name
                        .ok_or_else(|| {
                            ProtocolError::invalid_request(
                                "chat",
                                "tool_choice requires a function name",
                            )
                        })?
                        .to_string(),
                }),
                other => Err(ProtocolError::invalid_request(
                    "chat",
                    format!("unsupported tool_choice type `{other}`"),
                )),
            }
        }
        _ => Err(ProtocolError::invalid_request(
            "chat",
            "tool_choice must be a string or object",
        )),
    }
}

fn parse_response_format(value: Option<&Value>) -> Result<Option<ResponseFormat>, ProtocolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_request("chat", "response_format must be an object")
    })?;

    Ok(Some(match get_str(obj, "type").unwrap_or("text") {
        "text" => ResponseFormat::Text,
        "json_object" => ResponseFormat::JsonObject,
        "json_schema" => {
            let schema_obj = obj
                .get("json_schema")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    ProtocolError::invalid_request(
                        "chat",
                        "response_format.json_schema must be an object",
                    )
                })?;
            let schema = schema_obj.get("schema").cloned().ok_or_else(|| {
                ProtocolError::invalid_request(
                    "chat",
                    "response_format.json_schema.schema is required",
                )
            })?;
            ResponseFormat::JsonSchema {
                name: get_string(schema_obj, "name"),
                schema,
                strict: get_bool(schema_obj, "strict"),
            }
        }
        _ => return Ok(None),
    }))
}

pub fn encode_request(request: &UnifiedRequest) -> Result<Value, ProtocolError> {
    if request.top_k.is_some() {
        return Err(ProtocolError::unsupported("top_k", "chat"));
    }

    let mut obj = Map::new();
    obj.insert("model".into(), Value::String(request.model.clone()));

    let mut messages = Vec::new();
    if !request.system.is_empty() {
        if request
            .system
            .iter()
            .any(|part| !matches!(part, ContentPart::Text { .. }))
        {
            return Err(ProtocolError::unsupported(
                "non-text system content",
                "chat",
            ));
        }
        messages.push(json!({
            "role": "system",
            "content": text_content(&request.system),
        }));
    }

    for message in &request.messages {
        match message.role {
            Role::System | Role::Developer => {
                if message
                    .content
                    .iter()
                    .any(|part| !matches!(part, ContentPart::Text { .. }))
                {
                    return Err(ProtocolError::unsupported(
                        "non-text system content",
                        "chat",
                    ));
                }
                messages.push(json!({
                    "role": "system",
                    "content": text_content(&message.content),
                }));
            }
            Role::User => {
                messages.push(json!({
                    "role": "user",
                    "content": encode_user_content(&message.content)?,
                }));
            }
            Role::Assistant => {
                let text = text_content(&message.content);
                let reasoning = reasoning_text(&message.content);
                let mut message_obj = Map::new();
                message_obj.insert("role".into(), json!("assistant"));
                message_obj.insert(
                    "content".into(),
                    if text.is_empty() {
                        Value::Null
                    } else {
                        Value::String(text)
                    },
                );
                if !reasoning.is_empty() {
                    message_obj.insert("reasoning_content".into(), Value::String(reasoning));
                }
                let tool_calls: Vec<Value> = message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::ToolUse { id, name, input } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::to_string(input)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            }
                        })),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    message_obj.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(message_obj));
            }
            Role::Tool => {
                for part in &message.content {
                    if let ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = part
                    {
                        ensure_text_only(content, "chat")?;
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": text_content(content),
                        }));
                    }
                }
            }
        }
    }
    obj.insert("messages".into(), Value::Array(messages));

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut function = Map::new();
                function.insert("name".into(), json!(tool.name));
                if let Some(description) = &tool.description {
                    function.insert("description".into(), json!(description));
                }
                function.insert("parameters".into(), tool.parameters.clone());
                if let Some(strict) = tool.strict {
                    function.insert("strict".into(), json!(strict));
                }
                json!({"type": "function", "function": Value::Object(function)})
            })
            .collect();
        obj.insert("tools".into(), Value::Array(tools));
        obj.insert(
            "tool_choice".into(),
            encode_tool_choice(&request.tool_choice),
        );
    }

    if let Some(parallel) = request.parallel_tool_calls {
        obj.insert("parallel_tool_calls".into(), json!(parallel));
    }
    if let Some(max_tokens) = request.max_tokens {
        obj.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        obj.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        obj.insert("top_p".into(), json!(top_p));
    }
    if !request.stop.is_empty() {
        obj.insert("stop".into(), json!(request.stop));
    }
    if request.stream {
        obj.insert("stream".into(), json!(true));
    }
    if let Some(reasoning) = &request.reasoning {
        let effort = reasoning
            .effort
            .or_else(|| reasoning.budget_tokens.map(effort_for_budget));
        if let Some(effort) = effort {
            obj.insert("reasoning_effort".into(), json!(effort_name(effort)));
        }
    }
    if let Some(format) = &request.response_format {
        obj.insert("response_format".into(), encode_response_format(format)?);
    }
    if let Some(user) = &request.metadata_user_id {
        obj.insert("user".into(), json!(user));
    }
    if let Some(n) = request.n {
        obj.insert("n".into(), json!(n));
    }

    Ok(Value::Object(obj))
}

fn encode_user_content(parts: &[ContentPart]) -> Result<Value, ProtocolError> {
    if parts
        .iter()
        .all(|part| matches!(part, ContentPart::Text { .. }))
    {
        return Ok(Value::String(text_content(parts)));
    }

    let mut result = Vec::new();
    for part in parts {
        match part {
            ContentPart::Text { text } => result.push(json!({"type": "text", "text": text})),
            ContentPart::Image { source } => result.push(json!({
                "type": "image_url",
                "image_url": {"url": image_url(source)},
            })),
            _ => return Err(ProtocolError::unsupported("non-text user content", "chat")),
        }
    }
    Ok(Value::Array(result))
}

fn encode_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": {"name": name},
        }),
    }
}

fn encode_response_format(format: &ResponseFormat) -> Result<Value, ProtocolError> {
    match format {
        ResponseFormat::Text => Ok(json!({"type": "text"})),
        ResponseFormat::JsonObject => Ok(json!({"type": "json_object"})),
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => Ok(json!({
            "type": "json_schema",
            "json_schema": {
                "name": name.clone().unwrap_or_else(|| "response".to_string()),
                "schema": schema,
                "strict": strict,
            }
        })),
    }
}

pub fn parse_response(value: &Value) -> Result<UnifiedResponse, ProtocolError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ProtocolError::invalid_response("chat", "response must be a JSON object"))?;

    let id = get_str(obj, "id").unwrap_or_default().to_string();
    let model = get_str(obj, "model").unwrap_or_default().to_string();
    let created_at = obj.get("created").and_then(Value::as_i64).unwrap_or(0);

    let choice = obj
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object);

    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(choice) = choice {
        if let Some(message) = choice.get("message").and_then(Value::as_object) {
            if let Some(reasoning) =
                get_str(message, "reasoning_content").or_else(|| get_str(message, "reasoning"))
            {
                if !reasoning.is_empty() {
                    content.push(ContentPart::Reasoning {
                        text: reasoning.to_string(),
                        signature: None,
                    });
                }
            }
            content.extend(parse_output_content(message.get("content"))?);

            if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in tool_calls {
                    let call = call.as_object().ok_or_else(|| {
                        ProtocolError::invalid_response("chat", "tool_call must be an object")
                    })?;
                    let id = get_str(call, "id").unwrap_or_default().to_string();
                    let function =
                        call.get("function")
                            .and_then(Value::as_object)
                            .ok_or_else(|| {
                                ProtocolError::invalid_response(
                                    "chat",
                                    "tool_call.function must be an object",
                                )
                            })?;
                    let name = get_str(function, "name").unwrap_or_default().to_string();
                    let input = parse_arguments(get_str(function, "arguments").unwrap_or("{}"));
                    content.push(ContentPart::ToolUse { id, name, input });
                }
            }
        }

        stop_reason = get_str(choice, "finish_reason").map(parse_finish_reason);
    }

    let usage = obj
        .get("usage")
        .and_then(Value::as_object)
        .map(parse_usage)
        .unwrap_or_default();

    Ok(UnifiedResponse {
        id,
        model,
        created_at,
        content,
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

pub(crate) fn parse_finish_reason(value: &str) -> StopReason {
    match value {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        _ => StopReason::Other,
    }
}

pub(crate) fn parse_usage(usage: &Map<String, Value>) -> Usage {
    let prompt = get_u64(usage, "prompt_tokens");
    let output = get_u64(usage, "completion_tokens");
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| get_u64(details, "cached_tokens"));
    let input = match (prompt, cached) {
        (Some(prompt), Some(cached)) => Some(prompt.saturating_sub(cached)),
        (Some(prompt), None) => Some(prompt),
        _ => None,
    };
    let total = get_u64(usage, "total_tokens").or_else(|| match (prompt, output) {
        (Some(prompt), Some(output)) => Some(prompt + output),
        _ => None,
    });

    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cached_tokens: cached,
        cache_creation_tokens: None,
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| get_u64(details, "reasoning_tokens")),
    }
}

pub fn encode_response(response: &UnifiedResponse) -> Result<Value, ProtocolError> {
    let text = text_content(&response.content);
    let reasoning = reasoning_text(&response.content);

    let mut message = Map::new();
    message.insert("role".into(), json!("assistant"));
    message.insert(
        "content".into(),
        if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        },
    );
    if !reasoning.is_empty() {
        message.insert("reasoning_content".into(), Value::String(reasoning));
    }

    let tool_calls: Vec<Value> = response
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(input)
                        .unwrap_or_else(|_| "{}".to_string()),
                }
            })),
            _ => None,
        })
        .collect();
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(tool_calls));
    }

    let finish_reason = response
        .stop_reason
        .map(|reason| match reason {
            StopReason::EndTurn | StopReason::StopSequence => "stop",
            StopReason::MaxTokens => "length",
            StopReason::ToolUse => "tool_calls",
            StopReason::ContentFilter => "content_filter",
            StopReason::Other => "stop",
        })
        .unwrap_or("stop");

    let mut usage = Map::new();
    let prompt_tokens = response.usage.input_tokens.map(|input| {
        input
            + response.usage.cached_tokens.unwrap_or(0)
            + response.usage.cache_creation_tokens.unwrap_or(0)
    });
    if let Some(value) = prompt_tokens {
        usage.insert("prompt_tokens".into(), json!(value));
    }
    if let Some(value) = response.usage.output_tokens {
        usage.insert("completion_tokens".into(), json!(value));
    }
    if let Some(value) = response.usage.total_tokens.or_else(|| {
        prompt_tokens
            .zip(response.usage.output_tokens)
            .map(|(input, output)| input + output)
    }) {
        usage.insert("total_tokens".into(), json!(value));
    }
    if response.usage.cached_tokens.is_some() || response.usage.reasoning_tokens.is_some() {
        if let Some(cached) = response.usage.cached_tokens {
            usage.insert(
                "prompt_tokens_details".into(),
                json!({"cached_tokens": cached}),
            );
        }
        if let Some(reasoning) = response.usage.reasoning_tokens {
            usage.insert(
                "completion_tokens_details".into(),
                json!({"reasoning_tokens": reasoning}),
            );
        }
    }

    Ok(json!({
        "id": response.id,
        "object": "chat.completion",
        "created": response.created_at,
        "model": response.model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
        "usage": Value::Object(usage),
    }))
}
