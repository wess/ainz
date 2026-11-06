use super::*;
use crate::context::{compaction_chunks, message_bytes};

/// How many messages each further attempt at making room keeps. Keeping one is what lets the
/// next prompt through: a large message leaves the tail as soon as it is not the latest thing
/// said, instead of failing every turn it takes to scroll out of a longer one.
pub(super) const TAILS: [usize; 2] = [2, 1];

/// Bytes to the token when sizing a summary request. The estimate's four is close for prose
/// and low for JSON, and a summary request that comes in low is refused like any other.
const SUMMARY_BYTES_PER_TOKEN: usize = 3;

/// What a request may carry once the reply has its room. A request whose prompt fits but
/// whose answer cannot is refused just the same.
pub(super) fn room(options: &RunOptions) -> usize {
  options.context_tokens - (options.context_tokens / 8).min(32_000)
}

/// Refuses a prompt no compaction could send. It stays out of the session because anything
/// kept there goes out with every request after it, and each of those would fail the same
/// way; compacting the history first would give that history up for nothing.
pub(super) fn admit(prompt: &Message, options: &RunOptions, fixed: usize) -> Result<()> {
  let limit = room(options);
  let estimate = (fixed + message_bytes(prompt)).div_ceil(4);
  if estimate > limit {
    bail!(
      "that message is about {estimate} tokens and only {limit} fit in a {} token context with \
       room left for a reply, so it was not sent and the conversation is as it was; put it in a \
       file in the workspace and ask about that instead, or raise context_tokens if this model \
       takes more",
      options.context_tokens
    );
  }
  Ok(())
}

/// Whether a provider refused a request for being too long. Every provider says it in its own
/// words, and a gateway passes them through, so this reads the sentence rather than a code.
pub(super) fn too_long(error: &anyhow::Error) -> bool {
  let text = format!("{error:#}").to_ascii_lowercase();
  [
    "prompt is too long",
    "input is too long",
    "input length",
    "context length",
    "context_length",
    "context window",
    "maximum context",
    "too many tokens",
    "request too large",
  ]
  .iter()
  .any(|phrase| text.contains(phrase))
}

impl<P: ChatProvider> Agent<P> {
  /// Returns whether anything was archived; nothing happens when too little is old enough.
  ///
  /// The archived history goes to the model in pieces that each fit, every piece summarised on
  /// top of the summary of the ones before it. Sending it whole is the request most likely to
  /// be refused for length, since it is only made once the history is already long, and a
  /// refusal here would repeat on every later turn.
  pub(super) async fn compact(
    &self,
    session: &mut Session,
    options: &RunOptions,
    total: &mut Usage,
    preserve: usize,
  ) -> Result<bool> {
    let Some((cursor, input, archived_messages)) = session.compaction_input(preserve)? else {
      return Ok(false);
    };
    let budget = room(options) / 2 * SUMMARY_BYTES_PER_TOKEN;
    let instructions = Message::text(
      Role::System,
      concat!(
        "Summarize the session transcript for another agent continuing the work. ",
        "Preserve decisions, constraints, file paths, commands and results, unresolved ",
        "errors, and the next concrete action. Omit conversational filler."
      ),
    );
    let mut summary = String::new();
    for chunk in compaction_chunks(&input, budget) {
      let transcript = if summary.is_empty() {
        chunk
      } else {
        format!("Summary of the session before this point:\n{summary}\n\n{chunk}")
      };
      let request = Message::text(Role::User, transcript);
      let reply = self
        .provider
        .complete(&[&instructions, &request], &[], &EventSink::default())
        .await?;
      accumulate(total, &reply.usage);
      summary = reply.message.content.unwrap_or_default();
      if summary.trim().is_empty() {
        bail!("context compaction returned an empty summary");
      }
    }
    if summary.is_empty() {
      return Ok(false);
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

#[cfg(test)]
mod tests {
  use super::too_long;

  #[test]
  fn a_refusal_for_length_is_recognised_in_each_provider_s_words() {
    for text in [
      "anthropic: prompt is too long: 214000 tokens > 200000 maximum",
      "litellm.BadRequestError: input length and `max_tokens` exceed context limit",
      "openai: This model's maximum context length is 128000 tokens",
      "context_length_exceeded",
      "Bedrock: Input is too long for requested model.",
    ] {
      assert!(too_long(&anyhow::anyhow!(text.to_string())), "{text}");
    }
    assert!(!too_long(&anyhow::anyhow!("401 Unauthorized")));
    assert!(!too_long(&anyhow::anyhow!(
      "Account is not entitled to this Marketplace product"
    )));
  }
}
