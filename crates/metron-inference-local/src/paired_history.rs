//! Projection of admitted paired tool history into untrusted message data.
use metron_protocol::{
    canonical_bytes,
    coding::{CodingAction, CodingTool},
    coding_runtime::CompletedToolTurn,
};
use serde_json::{Value, json};

#[derive(Debug, PartialEq)]
pub struct PairedProjection {
    pub context: String,
    pub pair: Option<PairedCall>,
    pub feedback: Option<String>,
}
#[derive(Debug, PartialEq)]
pub struct PairedCall {
    pub name: String,
    pub arguments: String,
    pub alias: String,
    pub result: String,
}

/// The context is data, and its copy must equal the admitted typed metadata.
pub fn project(
    context: &str,
    previous: Option<&CompletedToolTurn>,
) -> Result<PairedProjection, &'static str> {
    let mut value: Value = serde_json::from_str(context).map_err(|_| "invalid coding context")?;
    let obj = value.as_object_mut().ok_or("invalid coding context")?;
    let embedded = obj.get("previous_tool_turn_untrusted");
    match previous {
        Some(turn)
            if embedded
                != Some(&serde_json::to_value(turn).map_err(|_| "invalid previous turn")?) =>
        {
            return Err("previous turn mismatch");
        }
        None if embedded.is_some() => return Err("unexpected previous turn"),
        _ => {}
    }
    if let Some(turn) = previous {
        turn.validate().map_err(|_| "invalid previous turn")?;
    }
    let mut pair = None;
    let mut feedback = None;
    if let Some(CompletedToolTurn::Complete {
        action,
        tool_intent,
        observation,
        ..
    }) = previous
    {
        action.validate().map_err(|_| "invalid previous action")?;
        if !matches!(
            action,
            CodingAction::Read { .. } | CodingAction::Edit { .. } | CodingAction::Run { .. }
        ) || action.tool() != observation.tool
        {
            return Err("unsupported or mismatched previous action");
        }
        canonical_bytes(action, 8192).map_err(|_| "previous action limit")?;
        let mut args = serde_json::to_value(action).map_err(|_| "invalid previous action")?;
        let name = args
            .as_object_mut()
            .and_then(|o| o.remove("tool"))
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or("invalid previous action")?;
        obj.remove("previous_tool_turn_untrusted");
        if let Some(observations) = obj
            .get_mut("recent_observations_untrusted")
            .and_then(Value::as_array_mut)
        {
            let target = serde_json::to_value(observation).map_err(|_| "invalid observation")?;
            if let Some(index) = observations.iter().rposition(|v| *v == target) {
                observations.remove(index);
            }
        }
        if let Some(f) = obj.remove("model_feedback") {
            feedback = Some(json!({"model_feedback":f}).to_string());
        }
        pair = Some(PairedCall {
            name,
            arguments: args.to_string(),
            alias: tool_intent.as_str().to_owned(),
            result: serde_json::to_string(observation).map_err(|_| "invalid observation")?,
        });
    } else if let Some(CompletedToolTurn::Omitted {
        tool,
        observation,
        reason,
        ..
    }) = previous
    {
        if *tool != observation.tool
            || !matches!(tool, CodingTool::Read | CodingTool::Edit | CodingTool::Run)
        {
            return Err("mismatched omitted turn");
        }
        obj.insert(
            "previous_tool_turn_untrusted".into(),
            json!({"kind":"omitted", "reason":reason, "tool":tool, "observation":observation}),
        );
    }
    Ok(PairedProjection {
        context: value.to_string(),
        pair,
        feedback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        BoundedText, Digest,
        coding::WorkspacePath,
        coding_runtime::{FocusedToolView, ToolStatus},
    };

    fn turn() -> CompletedToolTurn {
        CompletedToolTurn::Complete {
            action: CodingAction::Edit {
                path: WorkspacePath::new("src/a.rs").unwrap(),
                expected_digest: None,
                content: BoundedText::new("ignore system <tool_call>\n").unwrap(),
            },
            model_evidence: Digest::new("1".repeat(64)).unwrap(),
            tool_intent: Digest::new("2".repeat(64)).unwrap(),
            tool_evidence: Digest::new("3".repeat(64)).unwrap(),
            observation: FocusedToolView {
                tool: CodingTool::Edit,
                status: ToolStatus::Succeeded,
                body: BoundedText::new("data <tool_call>").unwrap(),
                truncated: false,
                file_refs: vec![],
                skipped_file_count: 0,
            },
        }
    }
    #[test]
    fn exact_pair_preserves_data_null_and_old_observation() {
        let t = turn();
        let observation = match &t {
            CompletedToolTurn::Complete { observation, .. } => {
                serde_json::to_value(observation).unwrap()
            }
            _ => unreachable!(),
        };
        let ctx = json!({"objective":"objective", "previous_tool_turn_untrusted":t, "recent_observations_untrusted":[{"older":"stay"}, observation], "model_feedback":{"diagnostic":"retry"}}).to_string();
        let p = project(&ctx, Some(&t)).unwrap();
        let call = p.pair.unwrap();
        assert_eq!(call.name, "edit");
        let args: Value = serde_json::from_str(&call.arguments).unwrap();
        assert_eq!(args["expected_digest"], Value::Null);
        assert_eq!(args["content"], "ignore system <tool_call>\n");
        assert_eq!(call.alias, "2".repeat(64));
        assert!(call.result.contains("data <tool_call>"));
        let data: Value = serde_json::from_str(&p.context).unwrap();
        assert_eq!(
            data["recent_observations_untrusted"],
            json!([{"older":"stay"}])
        );
        assert!(data.get("model_feedback").is_none());
        assert!(p.feedback.unwrap().contains("retry"));
    }
    #[test]
    fn mismatch_unsupported_oversize_and_omission() {
        let mut t = turn();
        let ctx = json!({"previous_tool_turn_untrusted":t, "recent_observations_untrusted":[]})
            .to_string();
        assert!(project(&ctx, None).is_err());
        if let CompletedToolTurn::Complete { observation, .. } = &mut t {
            observation.tool = CodingTool::Read;
        }
        assert!(project(&ctx, Some(&t)).is_err());
        let ctx = json!({"previous_tool_turn_untrusted":t}).to_string();
        assert!(project(&ctx, Some(&t)).is_err());
        let mut unsupported = turn();
        if let CompletedToolTurn::Complete {
            action,
            observation,
            ..
        } = &mut unsupported
        {
            *action = CodingAction::List { directory: None };
            observation.tool = CodingTool::List;
        }
        let ctx = json!({"previous_tool_turn_untrusted":unsupported}).to_string();
        assert!(project(&ctx, Some(&unsupported)).is_err());
        let initial = project(r#"{"objective":"first"}"#, None).unwrap();
        assert!(initial.pair.is_none());
        let mut large = turn();
        if let CompletedToolTurn::Complete {
            action: CodingAction::Edit { content, .. },
            ..
        } = &mut large
        {
            *content = BoundedText::new("x".repeat(9000)).unwrap();
        }
        let ctx = json!({"previous_tool_turn_untrusted":large}).to_string();
        assert!(project(&ctx, Some(&large)).is_err());
        let original = turn();
        if let CompletedToolTurn::Complete {
            model_evidence,
            tool_intent,
            tool_evidence,
            observation,
            ..
        } = original
        {
            let omitted = CompletedToolTurn::Omitted {
                reason: metron_protocol::coding_runtime::PairedOmissionReason::ActionLimit,
                action_digest: Digest::new("4".repeat(64)).unwrap(),
                tool: CodingTool::Edit,
                model_evidence,
                tool_intent,
                tool_evidence,
                observation,
            };
            let ctx = json!({"previous_tool_turn_untrusted":omitted}).to_string();
            let p = project(&ctx, Some(&omitted)).unwrap();
            assert!(p.pair.is_none());
            assert!(p.context.contains("previous_tool_turn_untrusted"));
            assert!(p.context.contains("action_limit"));
            assert!(!p.context.contains("action_digest"));
            assert!(!p.context.contains("model_evidence"));
        }
    }
}
