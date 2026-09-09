export const copyText = async (text, message) => {
  const notice = document.getElementById("notice");
  try {
    await navigator.clipboard.writeText(text);
    notice.textContent = message;
  } catch {
    const fallback = document.getElementById("fallback");
    const field = document.getElementById("copytext");
    fallback.hidden = false;
    fallback.open = true;
    field.value = text;
    field.focus();
    field.select();
    notice.textContent = "Clipboard access failed. Copy the selected text below.";
  }
};
