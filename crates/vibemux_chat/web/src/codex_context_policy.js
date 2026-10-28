export function summarize_context(conversation = null) {
  const messages = Array.isArray(conversation?.messages)
    ? conversation.messages
    : [];
  const input_tokens = messages.reduce(
    (total, message) => total + (message?.usage?.input_tokens ?? 0),
    0,
  );
  const output_tokens = messages.reduce(
    (total, message) => total + (message?.usage?.output_tokens ?? 0),
    0,
  );
  const attachment_sources = [
    conversation?.attachments,
    ...messages.map((message) => message?.attachments),
  ];
  const attachments = [];
  const attachment_ids = new Set();

  for (const source of attachment_sources) {
    if (!Array.isArray(source)) continue;
    for (const attachment of source) {
      const attachment_id =
        attachment?.id ?? attachment?.path ?? attachment?.name;
      if (attachment_id === undefined || attachment_ids.has(attachment_id))
        continue;
      attachment_ids.add(attachment_id);
      attachments.push(attachment);
    }
  }

  return {
    message_count: messages.length,
    input_tokens,
    output_tokens,
    attachments,
    artifact: conversation?.artifact ?? null,
    model: conversation?.model ?? null,
    thinking: conversation?.thinking ?? false,
    created_at: conversation?.created_at ?? null,
  };
}
