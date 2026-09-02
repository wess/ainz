document.querySelectorAll('[data-copy]').forEach((button) => {
  button.addEventListener('click', async () => {
    const code = button.parentElement.querySelector('code');
    if (!code) return;
    try {
      await navigator.clipboard.writeText(code.textContent);
      button.textContent = 'copied';
    } catch {
      button.textContent = 'select and copy';
    }
    window.setTimeout(() => { button.textContent = 'copy'; }, 1200);
  });
});
