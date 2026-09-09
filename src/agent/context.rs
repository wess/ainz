use super::*;
use crate::context::transcript;

impl<P: ChatProvider> Agent<P> {
  // returns whether anything was archived; nothing happens when too little is old enough
  pub(super) async fn compact(
    &self,
    session: &mut Session,
    options: &RunOptions,
    total: &mut Usage,
  ) -> Result<bool> {
    let Some((cursor, input, archived_messages)) =
      session.compaction_input(options.preserve_messages)?
    else {
      return Ok(false);
    };
    let request = vec![
      Message::text(
        Role::System,
        concat!(
          "Summarize the session transcript for another agent continuing the work. ",
          "Preserve decisions, constraints, file paths, commands and results, unresolved ",
          "errors, and the next concrete action. Omit conversational filler."
        ),
      ),
      Message::text(Role::User, transcript(&input)),
    ];
    let reply = self
      .provider
      .complete(&request, &[], &EventSink::default())
      .await?;
    accumulate(total, &reply.usage);
    let summary = reply.message.content.unwrap_or_default();
    if summary.trim().is_empty() {
      bail!("context compaction returned an empty summary");
    }
    session.record_summary(cursor, summary.clone())?;
    // the one moment where not having written something down costs immediately
    if let Some(nudge) = options.memory_nudge.as_deref() {
      session.append(Message::text(Role::System, nudge));
    }
    self.events.emit(Event::Compaction {
      archived_messages,
      summary,
    });
    Ok(true)
  }
}
