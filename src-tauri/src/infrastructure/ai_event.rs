use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiRuntimeEvent {
    RunStarted {
        run_id: String,
        sequence: u64,
    },
    TextDelta {
        run_id: String,
        sequence: u64,
        delta: String,
    },
    ToolStarted {
        run_id: String,
        sequence: u64,
        tool_call_id: String,
        tool_name: String,
    },
    ToolCompleted {
        run_id: String,
        sequence: u64,
        tool_call_id: String,
        tool_name: String,
        success: bool,
    },
    ApprovalRequired {
        run_id: String,
        sequence: u64,
        capability: String,
        operation: String,
    },
    UsageUpdated {
        run_id: String,
        sequence: u64,
        input_tokens: u64,
        output_tokens: u64,
    },
    RunCompleted {
        run_id: String,
        sequence: u64,
    },
    RunBlocked {
        run_id: String,
        sequence: u64,
    },
    RunFailed {
        run_id: String,
        sequence: u64,
        error_code: String,
    },
    RunCancelled {
        run_id: String,
        sequence: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::AiRuntimeEvent;

    #[test]
    fn runtime_events_use_stable_tagged_wire_format() {
        let event = AiRuntimeEvent::TextDelta {
            run_id: "run-1".to_string(),
            sequence: 2,
            delta: "hello".to_string(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["run_id"], "run-1");
        assert_eq!(value["sequence"], 2);
    }
}
