use super::*;
use crate::{PermissionMode, tool::ToolContext};

impl<P: ChatProvider> Agent<P> {
  pub(super) async fn run_tool(
    &self,
    call: &ToolCall,
    options: &RunOptions,
    session_id: uuid::Uuid,
  ) -> (String, bool, bool) {
    let Some(tool) = self.tools.get(&call.name) else {
      return (format!("unknown tool: {}", call.name), true, false);
    };
    if let Err(error) = tool.validate(&call.arguments) {
      return (format!("invalid arguments: {error:#}"), true, false);
    }
    let subject = match crate::permission::subject(&self.workspace, call).await {
      Ok(subject) => subject,
      Err(error) => return (format!("{error:#}"), true, false),
    };
    let risk = tool.risk(&call.arguments);
    // a standing rule answers before anyone is asked, in every mode: it is the same decision,
    // made once already
    let rules =
      match crate::permission::normalize(&options.rules, &self.workspace, &call.name).await {
        Ok(rules) => rules,
        Err(error) => return (format!("invalid permission rule: {error:#}"), true, false),
      };
    let ruled = rules.decide(&call.name, subject.as_deref());
    let allowed = match (ruled, options.permissions) {
      (Some(decided), _) => decided,
      (None, PermissionMode::Auto) => true,
      (None, PermissionMode::ReadOnly) => risk == Risk::Read,
      (None, PermissionMode::Ask) => risk == Risk::Read || (self.approver)(call, risk).await,
    };
    if !allowed {
      return (
        format!("permission denied for {} ({risk:?})", call.name),
        true,
        false,
      );
    }
    // a call refused above never reaches this, so a pre_tool hook only ever sees work that
    // was already going to run
    if let Err(reason) = options
      .hooks
      .pre_tool(&self.workspace, session_id, call, &self.events)
      .await
    {
      return (reason, true, false);
    }
    let context = ToolContext {
      workspace: self.workspace.clone(),
      session_id,
      max_output_bytes: options.max_output_bytes,
      progress: Some((self.events.clone(), call.id.clone())),
    };
    let (output, error) = match tool.execute(&context, call.arguments.clone()).await {
      Ok(output) => (output, false),
      Err(error) => (format!("{error:#}"), true),
    };
    (output, error, true)
  }
}
