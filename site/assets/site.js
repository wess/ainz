const install = document.querySelector('[data-install]');
if (install) {
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);
  install.textContent = mac
    ? 'brew install wess/packages/agentx'
    : "curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/wess/agentx/v0.1.0/install.sh | sh";
  const marker = document.querySelector('[data-os]');
  if (marker) marker.textContent = mac ? 'macOS' : 'Linux';
}

document.querySelectorAll('[data-copy]').forEach((button) => {
  button.addEventListener('click', async () => {
    const code = button.parentElement.querySelector('code');
    if (!code) return;
    await navigator.clipboard.writeText(code.textContent);
    button.textContent = 'copied';
    window.setTimeout(() => { button.textContent = 'copy'; }, 1200);
  });
});
